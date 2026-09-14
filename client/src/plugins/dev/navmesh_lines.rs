use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::nvm::JMXVNVM;
use crate::plugins::cursor::GameCursorCamera;
use crate::plugins::dev::render_debug::RenderDebugSettings;
use crate::plugins::map::terrain::TerrainNavMeshData;
use crate::plugins::map::terrain::REGION_SIZE;
use crate::plugins::nav::{
    NavLocation, NavMeshRaycast, NavObjectGrid, NavStep, ObjectNavMesh, OBJECT_BLOCK_Y_BAND,
};
use crate::plugins::player::Player;
use bevy::color::palettes::basic::{BLUE, FUCHSIA, GREEN, RED, YELLOW};
use bevy::prelude::*;

/// Lift debug lines slightly above the terrain so they don't z-fight with it.
const LINE_LIFT: f32 = 1.0;
/// Max world-space length of one polyline segment; longer nav lines are subdivided
/// at this step so they follow the terrain instead of clipping through hills.
const SEGMENT_STEP: f32 = 40.0;
/// Object nav mesh triangle wireframe colour — dimmed so the flag-coloured
/// edges drawn on top of it stay readable.
const CELL_COLOR: Color = Color::srgba(0.0, 0.7, 0.9, 0.35);
/// An edge movement can cross.
const PASSABLE_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
/// An edge that blocks, and that the movement query will actually test.
const BLOCKING_COLOR: Color = Color::srgb(1.0, 0.15, 0.1);
/// An edge that blocks in the data but sits outside the storey band, so it is
/// skipped — solid on paper, ignored in practice.
const BLOCKING_IGNORED_COLOR: Color = Color::srgb(0.35, 0.0, 0.0);
/// An object outline edge carrying the `Global` flag — the transfer points the
/// data marks for handing a mover between an object and its surroundings.
const GLOBAL_EDGE_COLOR: Color = Color::srgb(1.0, 0.95, 0.1);
/// Radius around the player the diagnostic log summarises.
const DIAGNOSTIC_RADIUS: f32 = 200.0;
/// Length of the synthetic movement steps the diagnostic drives through the
/// real query — long enough to cross a wall, short enough to stay local.
const PROBE_STEP: f32 = 30.0;
/// How far above a mover its [`NavLocation`] marker floats — clear of a
/// character body (~18 units tall).
const MARKER_HEIGHT: f32 = 24.0;
const MARKER_RADIUS: f32 = 1.5;
/// Radius the KeyU snapshot lists individual edges within.
const SNAPSHOT_RADIUS: f32 = 60.0;
/// How many edges of each kind the snapshot prints.
const SNAPSHOT_EDGES: usize = 14;
/// How many passable outline edges the snapshot drives a real step across.
/// Fewer than [`SNAPSHOT_EDGES`]: each probe runs the full movement query.
const SNAPSHOT_PROBES: usize = 6;
/// How far past an outline edge the boarding probe aims, so the synthetic step
/// properly crosses it rather than landing on it (crossings are strict — see
/// `segments_intersect`).
const PROBE_OVERSHOOT: f32 = 5.0;

pub fn draw_debug_lines_for_nav_mesh(
    mut settings: ResMut<RenderDebugSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut gizmos: Gizmos,
    // InheritedVisibility, not ViewVisibility: a region root carries no mesh of
    // its own (its merge groups are children), so the render-side visibility
    // check never marks it visible — the same reason the object gizmo below
    // silently drew nothing.
    navmesh_query: Query<(&TerrainNavMeshData, &InheritedVisibility, &GlobalTransform)>,
    nav_meshes: Res<Assets<JMXVNVM>>,
) {
    if keys.just_pressed(KeyCode::KeyT) {
        let render_navmesh = !settings.render_navmesh;
        settings.render_navmesh = render_navmesh;
    }

    if !settings.render_navmesh {
        return;
    }

    for (nav_mesh_data, visibility, transform) in navmesh_query.iter() {
        if !visibility.get() {
            continue;
        }

        let Some(nav_mesh) = nav_meshes.get(&nav_mesh_data.0) else {
            // Still loading.
            continue;
        };

        let origin = transform.translation();

        // Quadtree cells (walkable rectangles).
        for cell in nav_mesh.quad_cell_list.items.iter() {
            let rect = &cell.rectangle;
            let corners = [
                rect.min,
                Vec2::new(rect.max.x, rect.min.y),
                rect.max,
                Vec2::new(rect.min.x, rect.max.y),
            ];
            for i in 0..4 {
                draw_nav_line(
                    &mut gizmos,
                    nav_mesh,
                    origin,
                    corners[i],
                    corners[(i + 1) % 4],
                    BLUE.into(),
                );
            }
        }

        // Cell border edges shared between regions / on the region outline.
        for edge in nav_mesh.global_edge_list.0.iter() {
            let color = if edge.flag.is_blocked() { RED } else { GREEN };
            draw_nav_line(
                &mut gizmos,
                nav_mesh,
                origin,
                edge.line.0,
                edge.line.1,
                color.into(),
            );
        }

        // Cell border edges inside the region.
        for edge in nav_mesh.internal_edge_list.0.iter() {
            let color = if edge.flag.is_blocked() { RED } else { YELLOW };
            draw_nav_line(
                &mut gizmos,
                nav_mesh,
                origin,
                edge.line.0,
                edge.line.1,
                color.into(),
            );
        }
    }
}

/// Draws every loaded map object's nav mesh: walkable triangles plus every
/// outline and inline edge, coloured by whether it blocks movement.
///
/// - **blue** — passable, movement crosses it freely
/// - **red** — blocks, and is close enough to the player's height that the
///   movement query will actually test it
/// - **dark red** — blocks in the data, but sits outside
///   [`OBJECT_BLOCK_Y_BAND`] of the player and is therefore *skipped* right now
///
/// That third colour is the point. A wall drawn dark red is one the data says
/// is solid and the movement code is ignoring, which looks identical to a
/// missing wall unless it is drawn differently.
pub fn draw_object_nav_meshes(
    mut settings: ResMut<RenderDebugSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut gizmos: Gizmos,
    // InheritedVisibility, not ViewVisibility: the nav mesh lives on the
    // resource *root*, which carries no mesh of its own, so the render-side
    // visibility check never marks it visible and the gizmo drew nothing.
    objects: Query<(&ObjectNavMesh, &InheritedVisibility, &GlobalTransform)>,
    players: Query<&GlobalTransform, With<Player>>,
    bms_meshes: Res<Assets<JMXVBMS>>,
) {
    if keys.just_pressed(KeyCode::KeyY) {
        settings.render_object_navmesh = !settings.render_object_navmesh;
    }

    if !settings.render_object_navmesh {
        return;
    }

    let player_y = players.single().map(|t| t.translation().y).ok();

    for (object_nav, visibility, transform) in objects.iter() {
        if !visibility.get() {
            continue;
        }

        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };

            let lift = Vec3::Y * LINE_LIFT;
            let to_world = |local: Vec3| transform.transform_point(local) + lift;

            // Walkable triangles, dimmed so the edges drawn over them read.
            for cell in &nav.cells {
                let (a, b, c) = nav.cell_triangle(cell);
                let (a, b, c) = (to_world(a), to_world(b), to_world(c));
                gizmos.line(a, b, CELL_COLOR);
                gizmos.line(b, c, CELL_COLOR);
                gizmos.line(c, a, CELL_COLOR);
            }

            for edge in nav.outline_edges.iter().chain(nav.inline_edges.iter()) {
                let src = to_world(nav.vertices[edge.src_vertex as usize].position);
                let dst = to_world(nav.vertices[edge.dst_vertex as usize].position);

                let color = if !edge.flag.is_blocked() {
                    PASSABLE_COLOR
                } else {
                    // Mirrors the filter in `blocked_edge_crossed`, which keeps
                    // an upper storey's walls from blocking the floor below.
                    let in_band = player_y
                        .is_none_or(|y| (0.5 * (src.y + dst.y) - y).abs() <= OBJECT_BLOCK_Y_BAND);
                    if in_band {
                        BLOCKING_COLOR
                    } else {
                        BLOCKING_IGNORED_COLOR
                    }
                };
                gizmos.line(src, dst, color);
            }
        }
    }
}

/// Draws every loaded map object's **global** outline edges in yellow, always —
/// no toggle, no distance limit.
///
/// These are the edges the data flags `Global` (bit 3), and they are the only
/// place the format marks as a hand-off between an object and what surrounds
/// it. Nothing links them to the terrain: `LinkEdge` joins one object's global
/// edge to *another object's*, never to a terrain cell, so the client has to
/// recover the object↔terrain relation itself. The one thing the terrain side
/// offers is `NavCellQuad`'s object index list — which cell references which
/// object — and that is currently parsed and unused
/// (`assets/nvm/nav_cell_quad.rs`).
///
/// Drawn separately from [`draw_object_nav_meshes`] and on its own because the
/// question "where is this object supposed to hand over?" needs answering while
/// walking around normally, not only with the full nav overlay on.
pub fn draw_object_global_edges(
    mut gizmos: Gizmos,
    // InheritedVisibility, not ViewVisibility: the nav mesh lives on the
    // resource *root*, which carries no mesh of its own.
    objects: Query<(&ObjectNavMesh, &InheritedVisibility, &GlobalTransform)>,
    bms_meshes: Res<Assets<JMXVBMS>>,
    active_dungeon: Option<Res<crate::plugins::dungeon::ActiveDungeon>>,
) {
    // Inside a dungeon every room mesh carries global edges (they mark the
    // block-to-block hand-offs), so the always-on overlay would paint yellow
    // edging across every floor. The overworld question this overlay answers
    // ("where does this object hand over?") doesn't arise there — the full
    // nav overlay still shows dungeon edges when toggled on.
    if active_dungeon.is_some() {
        return;
    }
    for (object_nav, visibility, transform) in objects.iter() {
        if !visibility.get() {
            continue;
        }

        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };

            let lift = Vec3::Y * LINE_LIFT;
            for edge in nav.outline_edges.iter().filter(|e| e.flag.is_global()) {
                let src =
                    transform.transform_point(nav.vertices[edge.src_vertex as usize].position);
                let dst =
                    transform.transform_point(nav.vertices[edge.dst_vertex as usize].position);
                gizmos.line(src + lift, dst + lift, GLOBAL_EDGE_COLOR);
            }
        }
    }
}

/// Shows which nav surface each mover is tracking: a marker above it, coloured
/// by state, plus a line down to the object it claims to be standing on.
///
/// Movement is a state machine over [`NavLocation`] and is otherwise entirely
/// invisible — this is the gizmo that says whether a mover crossing a bridge
/// actually transferred onto the deck or is still being resolved against the
/// terrain underneath.
pub fn draw_nav_location(
    settings: Res<RenderDebugSettings>,
    mut gizmos: Gizmos,
    movers: Query<(&GlobalTransform, &NavLocation)>,
    objects: Query<&GlobalTransform, With<ObjectNavMesh>>,
) {
    if !settings.render_object_navmesh {
        return;
    }

    for (transform, location) in movers.iter() {
        let head = transform.translation() + Vec3::Y * MARKER_HEIGHT;
        let color = match location {
            NavLocation::Unresolved => Color::from(RED),
            NavLocation::Terrain => Color::from(GREEN),
            NavLocation::OnObject { .. } => Color::from(FUCHSIA),
        };
        gizmos.line(transform.translation(), head, color);
        gizmos.sphere(head, MARKER_RADIUS, color);

        // Tie the mover to the object it thinks it is on, so a wrong transfer
        // (the far bridge part, a building next door) is obvious.
        if let Some(object) = location.object() {
            if let Ok(object_transform) = objects.get(object) {
                gizmos.line(head, object_transform.translation(), color);
            }
        }
    }
}

/// One-shot detailed dump of the nav geometry immediately around the player,
/// on **KeyU**.
///
/// The periodic log summarises counts, which has repeatedly proved too coarse:
/// "202 blocked edges nearby" is true both when a wall fences the player in and
/// when every one of those edges is fifty units away. This prints the actual
/// edges — offsets from the player, height, length, and whether the storey band
/// keeps them — so the barrier's real shape can be read off rather than guessed.
pub fn dump_nav_snapshot(
    keys: Res<ButtonInput<KeyCode>>,
    players: Query<(&GlobalTransform, &NavLocation), With<Player>>,
    objects: Query<(Entity, &ObjectNavMesh, &GlobalTransform, Option<&Name>)>,
    regions: Query<(&TerrainNavMeshData, &GlobalTransform)>,
    nav_meshes: Res<Assets<JMXVNVM>>,
    bms_meshes: Res<Assets<JMXVBMS>>,
    nav: NavMeshRaycast,
    grid: Option<Res<NavObjectGrid>>,
) {
    if !keys.just_pressed(KeyCode::KeyU) {
        return;
    }
    let Ok((transform, location)) = players.single() else {
        return;
    };
    let player = transform.translation();
    let player_xz = Vec2::new(player.x, player.z);

    info!(
        "nav snapshot @ ({:.1},{:.1},{:.1}) loc={:?} — offsets are (dx,dz) from the player, \
         +x/+z are render axes",
        player.x, player.y, player.z, location
    );

    // What the geometry alone says this position is on, ignoring the tracked
    // state. A mismatch with `loc` above is the whole bug class: `validated()`
    // only re-resolves an `Unresolved` location, so a mover that entered an
    // object's footprint without registering a crossing keeps a stale
    // `Terrain` forever and stands at terrain height inside the object.
    let resolved = nav.resolve_location(player);
    info!(
        "  resolve_location here = {:?}{}",
        resolved,
        if resolved == *location {
            ""
        } else {
            "   <<< MISMATCH: tracked location is stale"
        }
    );

    // Per-object cell inventory, computed here rather than through the nav
    // module's raycast. When `resolve_location` says Terrain but an object's
    // outline edge is two units away, the question is whether the object has
    // walkable cells over this spot at all — a data answer — or whether it has
    // them and the query fails to find them, which is a code answer. The nav
    // module cannot distinguish those; a second, independent computation can.
    //
    // Also reports whether `NavObjectGrid` returns the object here. Every
    // failing path (`objects_near`, `objects_over_segment`) is gated on that
    // broad phase, while this dump and the gizmo renderer walk the query
    // directly — so an object can be plainly visible and drawn, yet invisible
    // to every movement query.
    let in_grid = grid
        .as_ref()
        .map(|g| g.candidates(player_xz).to_vec())
        .unwrap_or_default();
    for (entity, object_nav, object_transform, name) in objects.iter() {
        let mut cells = 0usize;
        let mut over_player: Vec<f32> = Vec::new();
        let mut near = false;
        let mut edge_near = false;

        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };
            cells += nav.cells.len();

            // Edge proximity is tracked separately from cell proximity so the
            // two can disagree out loud. An object listing a door 0.1 units
            // away but no cell beside it is a real and specific finding; if it
            // simply vanished from this list, that finding would read as an
            // absent line and be indistinguishable from a bug in this dump.
            for edge in nav.outline_edges.iter().chain(nav.inline_edges.iter()) {
                let src = object_transform
                    .transform_point(nav.vertices[edge.src_vertex as usize].position);
                let dst = object_transform
                    .transform_point(nav.vertices[edge.dst_vertex as usize].position);
                if distance_to_segment(player_xz, Vec2::new(src.x, src.z), Vec2::new(dst.x, dst.z))
                    <= SNAPSHOT_RADIUS
                {
                    edge_near = true;
                    break;
                }
            }

            for cell in &nav.cells {
                let (a, b, c) = nav.cell_triangle(cell);
                let (a, b, c) = (
                    object_transform.transform_point(a),
                    object_transform.transform_point(b),
                    object_transform.transform_point(c),
                );
                let tri = [
                    Vec2::new(a.x, a.z),
                    Vec2::new(b.x, b.z),
                    Vec2::new(c.x, c.z),
                ];
                // Distance to the triangle's *edges*, not to its corners. These
                // meshes have 130-unit-plus triangles, so a cell can contain
                // the player while every one of its vertices is out of range —
                // a corner test silently drops exactly the cells that matter.
                if (0..3).any(|i| {
                    distance_to_segment(player_xz, tri[i], tri[(i + 1) % 3]) <= SNAPSHOT_RADIUS
                }) {
                    near = true;
                }
                // Barycentric containment in XZ, then the plane height there —
                // the same question the downward probe asks, asked directly.
                if let Some(height) = triangle_height_at(player_xz, (a, b, c), tri) {
                    over_player.push(height);
                }
            }
        }

        // A cell covering the player always counts as near, whatever the
        // distance test concluded. Suppressing the one object the player is
        // standing inside is the exact failure this section exists to report.
        if !near && !edge_near && over_player.is_empty() {
            continue;
        }

        over_player.sort_by(|x, y| x.partial_cmp(y).unwrap());
        info!(
            "  object {:?} cells={} cells_near={} grid={} covering_player={} {} {}",
            entity,
            cells,
            if near { "yes" } else { "NO" },
            if in_grid.contains(&entity) {
                "YES"
            } else {
                "NO <<<"
            },
            over_player.len(),
            over_player
                .iter()
                .map(|h| format!("y={h:.1}(dy={:+.1})", h - player.y))
                .collect::<Vec<_>>()
                .join(" "),
            name.map_or_else(String::new, |n| n.to_string()),
        );
    }

    // Nearest blocked object edges, by distance from the player to the segment.
    let mut edges: Vec<(f32, String)> = Vec::new();
    for (_, object_nav, object_transform, name) in objects.iter() {
        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };
            for (edge, kind) in nav
                .outline_edges
                .iter()
                .map(|e| (e, "outline"))
                .chain(nav.inline_edges.iter().map(|e| (e, "inline")))
            {
                if !edge.flag.is_blocked() {
                    continue;
                }
                let a = object_transform
                    .transform_point(nav.vertices[edge.src_vertex as usize].position);
                let b = object_transform
                    .transform_point(nav.vertices[edge.dst_vertex as usize].position);
                let (a2, b2) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
                let distance = distance_to_segment(player_xz, a2, b2);
                if distance > SNAPSHOT_RADIUS {
                    continue;
                }
                let mid_y = 0.5 * (a.y + b.y);
                edges.push((
                    distance,
                    format!(
                        "  obj {:7} d={:6.1} ({:+6.1},{:+6.1})->({:+6.1},{:+6.1}) y={:7.1} dy={:+7.1} len={:5.1} {} {}",
                        kind,
                        distance,
                        a2.x - player.x,
                        a2.y - player.z,
                        b2.x - player.x,
                        b2.y - player.z,
                        mid_y,
                        mid_y - player.y,
                        a2.distance(b2),
                        if (mid_y - player.y).abs() <= OBJECT_BLOCK_Y_BAND { "TESTED " } else { "skipped" },
                        name.map_or_else(String::new, |n| n.to_string()),
                    ),
                ));
            }
        }
    }
    edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if edges.is_empty() {
        info!("  no blocked object edges within {SNAPSHOT_RADIUS:.0} units");
    }
    for (_, line) in edges.iter().take(SNAPSHOT_EDGES) {
        info!("{line}");
    }

    // Nearest *passable* outline edges — the doors, as opposed to the walls
    // above. Boarding an object is decided by crossing one of these
    // (`object_entered` -> `passable_outline_crossed`), so a snapshot that only
    // listed blocked edges could show why a mover was stopped but never why one
    // failed to get on. Every "walked under the bridge instead of onto it"
    // report is invisible without this section.
    let mut doors: Vec<(f32, Vec2, Vec2, String)> = Vec::new();
    for (_, object_nav, object_transform, name) in objects.iter() {
        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav) = &bms.navmesh else { continue };
            for edge in &nav.outline_edges {
                if edge.flag.is_blocked() {
                    continue;
                }
                let a = object_transform
                    .transform_point(nav.vertices[edge.src_vertex as usize].position);
                let b = object_transform
                    .transform_point(nav.vertices[edge.dst_vertex as usize].position);
                let (a2, b2) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
                let distance = distance_to_segment(player_xz, a2, b2);
                if distance > SNAPSHOT_RADIUS {
                    continue;
                }
                let mid_y = 0.5 * (a.y + b.y);
                // XZ normal of the edge, so the probes below can cross it in
                // both directions. Aiming "from the player through the edge"
                // only works when the player is outside the object; once
                // inside its footprint that direction points back out and
                // tests walking off, which is the opposite question.
                let along = (b2 - a2).normalize_or_zero();
                let normal = Vec2::new(-along.y, along.x);
                doors.push((
                    distance,
                    closest_point_on_segment(player_xz, a2, b2),
                    normal,
                    format!(
                        "  door outline d={:6.1} ({:+6.1},{:+6.1})->({:+6.1},{:+6.1}) y={:7.1} dy={:+7.1} len={:5.1} {} {} {}",
                        distance,
                        a2.x - player.x,
                        a2.y - player.z,
                        b2.x - player.x,
                        b2.y - player.z,
                        mid_y,
                        mid_y - player.y,
                        a2.distance(b2),
                        // Same storey filter `passable_outline_crossed` applies:
                        // an edge outside the band is not a door from here.
                        if (mid_y - player.y).abs() <= OBJECT_BLOCK_Y_BAND { "TESTED " } else { "skipped" },
                        if edge.flag.is_global() { "GLOBAL" } else { "      " },
                        name.map_or_else(String::new, |n| n.to_string()),
                    ),
                ));
            }
        }
    }
    doors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if doors.is_empty() {
        info!("  no passable object outline edges within {SNAPSHOT_RADIUS:.0} units");
    }
    for (_, _, _, line) in doors.iter().take(SNAPSHOT_EDGES) {
        info!("{line}");
    }

    // What actually happens on stepping through each door. Listing the edge
    // only says a door exists; driving the real `step()` across it says whether
    // the mover boards, is stopped, or — the bug this exists to catch — walks
    // straight through and stays on the terrain underneath.
    //
    // Both directions, always, and from either side of the edge rather than
    // from the player. Aiming outward from the player only asks the right
    // question while the player is *outside* the object; once inside its
    // footprint that direction points back out and tests walking off, which is
    // the opposite question — and inside-the-footprint is exactly the state
    // these reports come from.
    for (_, crossing, normal, _) in doors.iter().take(SNAPSHOT_PROBES) {
        if *normal == Vec2::ZERO {
            continue;
        }
        for side in [1.0_f32, -1.0] {
            let start = *crossing - *normal * side * PROBE_OVERSHOOT;
            let target = *crossing + *normal * side * PROBE_OVERSHOOT;
            let from = Vec3::new(start.x, player.y, start.y);
            let verdict = match nav.step(from, target, *location) {
                NavStep::Blocked => "BLOCKED".to_string(),
                NavStep::Unknown => "UNKNOWN (data not loaded)".to_string(),
                NavStep::Moved { position, location } => match location {
                    NavLocation::OnObject { .. } => format!(
                        "BOARDED y={:.1} (dy={:+.1})",
                        position.y,
                        position.y - player.y
                    ),
                    other => format!(
                        "{:?} y={:.1} (dy={:+.1})",
                        other,
                        position.y,
                        position.y - player.y
                    ),
                },
            };
            info!(
                "  probe ({:+6.1},{:+6.1}) -> ({:+6.1},{:+6.1}) {}",
                start.x - player.x,
                start.y - player.z,
                target.x - player.x,
                target.y - player.z,
                verdict
            );
        }
    }

    // Nearest blocked terrain edges, in region-local space.
    for (nav_data, region_transform) in regions.iter() {
        let origin = region_transform.translation();
        let local = Vec2::new(origin.x - player.x, player.z - origin.z);
        if !(0.0..=REGION_SIZE).contains(&local.x) || !(0.0..=REGION_SIZE).contains(&local.y) {
            continue;
        }
        let Some(nav_mesh) = nav_meshes.get(&nav_data.0) else {
            break;
        };
        let mut terrain: Vec<(f32, String)> = Vec::new();
        for (flag, line) in nav_mesh
            .internal_edge_list
            .0
            .iter()
            .map(|e| (&e.flag, e.line))
            .chain(
                nav_mesh
                    .global_edge_list
                    .0
                    .iter()
                    .map(|e| (&e.flag, e.line)),
            )
        {
            if !flag.is_blocked() {
                continue;
            }
            let distance = distance_to_segment(local, line.0, line.1);
            if distance > SNAPSHOT_RADIUS {
                continue;
            }
            terrain.push((
                distance,
                format!(
                    "  terrain d={:6.1} local ({:6.1},{:6.1})->({:6.1},{:6.1}) len={:5.1}",
                    distance,
                    line.0.x,
                    line.0.y,
                    line.1.x,
                    line.1.y,
                    line.0.distance(line.1),
                ),
            ));
        }
        terrain.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        info!("  player local=({:.1},{:.1})", local.x, local.y);
        if terrain.is_empty() {
            info!("  no blocked terrain edges within {SNAPSHOT_RADIUS:.0} units");
        }
        for (_, line) in terrain.iter().take(SNAPSHOT_EDGES) {
            info!("{line}");
        }
        break;
    }
}

/// Shortest distance from a point to a segment, in 2D.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    p.distance(closest_point_on_segment(p, a, b))
}

/// Height of the plane of triangle `world` at XZ position `p`, or `None` when
/// `p` lies outside the triangle's XZ projection.
///
/// Deliberately independent of the nav module's ray/triangle code: this exists
/// to check that code's answer, so sharing an implementation would defeat it.
fn triangle_height_at(p: Vec2, world: (Vec3, Vec3, Vec3), tri: [Vec2; 3]) -> Option<f32> {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let area = (b - a).perp_dot(c - a);
    if area.abs() < 1e-6 {
        return None;
    }
    let u = (b - p).perp_dot(c - p) / area;
    let v = (c - p).perp_dot(a - p) / area;
    let w = 1.0 - u - v;
    // A small negative tolerance so a point exactly on a shared edge is
    // reported once rather than falling through the crack between triangles.
    (u >= -1e-4 && v >= -1e-4 && w >= -1e-4).then(|| u * world.0.y + v * world.1.y + w * world.2.y)
}

/// The point on segment `ab` nearest `p`. The boarding probe aims through this
/// point, so a synthetic step crosses the edge where the mover would actually
/// meet it rather than at its midpoint.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-6 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab * t
}

/// Warns the moment a mover ends up standing in solid ground.
///
/// Independent of the periodic log and of the debug toggles: the interesting
/// instant is the one nobody is watching for, and judging "am I inside the wall
/// yet" by eye is exactly what makes these reports hard to act on. Fires on the
/// transition only, so it stays quiet while stuck.
pub fn warn_when_inside_solid_ground(
    players: Query<(&GlobalTransform, &NavLocation), With<Player>>,
    regions: Query<(&TerrainNavMeshData, &GlobalTransform)>,
    nav_meshes: Res<Assets<JMXVNVM>>,
    mut was_inside: Local<bool>,
) {
    let Ok((transform, location)) = players.single() else {
        return;
    };
    let player = transform.translation();

    let mut inside = false;
    let mut detail = String::new();
    for (nav_data, region_transform) in regions.iter() {
        let origin = region_transform.translation();
        let local = Vec2::new(origin.x - player.x, player.z - origin.z);
        if !(0.0..=REGION_SIZE).contains(&local.x) || !(0.0..=REGION_SIZE).contains(&local.y) {
            continue;
        }
        let Some(nav_mesh) = nav_meshes.get(&nav_data.0) else {
            break;
        };
        if !nav_mesh.quad_cell_list.is_walkable_at(local) {
            inside = true;
            detail = format!(
                "local=({:.0},{:.0}) cell={:?} tile_blocked={:?}",
                local.x,
                local.y,
                nav_mesh.quad_cell_list.cell_at(local),
                nav_mesh.tile_map.blocked_at(local),
            );
        }
        break;
    }

    if inside && !*was_inside {
        warn!(
            "nav: player entered SOLID ground at ({:.0},{:.1},{:.0}) loc={:?} {}",
            player.x, player.y, player.z, location, detail,
        );
    }
    *was_inside = inside;
}

pub fn log_nav_diagnostics(
    settings: Res<RenderDebugSettings>,
    time: Res<Time>,
    mut next_log: Local<f32>,
    players: Query<(&GlobalTransform, &NavLocation), With<Player>>,
    objects: Query<(Entity, &ObjectNavMesh, &GlobalTransform, Option<&Name>)>,
    regions: Query<(&TerrainNavMeshData, &GlobalTransform)>,
    nav_meshes: Res<Assets<JMXVNVM>>,
    bms_meshes: Res<Assets<JMXVBMS>>,
    nav: NavMeshRaycast,
) {
    if !settings.render_object_navmesh {
        return;
    }
    let now = time.elapsed_secs();
    if now < *next_log {
        return;
    }
    *next_log = now + 1.0;

    let Ok((player_transform, location)) = players.single() else {
        return;
    };
    let player = player_transform.translation();
    let player_xz = Vec2::new(player.x, player.z);

    // Drive the *real* movement query in eight directions. This is the whole
    // point of the log: it reports what the code that actually moves the player
    // decides, rather than what the data alone suggests it should.
    let probes: String = [
        ("N", Vec2::new(0.0, -1.0)),
        ("NE", Vec2::new(0.7, -0.7)),
        ("E", Vec2::new(1.0, 0.0)),
        ("SE", Vec2::new(0.7, 0.7)),
        ("S", Vec2::new(0.0, 1.0)),
        ("SW", Vec2::new(-0.7, 0.7)),
        ("W", Vec2::new(-1.0, 0.0)),
        ("NW", Vec2::new(-0.7, -0.7)),
    ]
    .iter()
    .map(|(name, dir)| {
        let verdict = match nav.step(player, player_xz + *dir * PROBE_STEP, *location) {
            NavStep::Blocked => "BLOCKED",
            NavStep::Unknown => "unknown",
            NavStep::Moved { .. } => "ok",
        };
        format!("{name}={verdict}")
    })
    .collect::<Vec<_>>()
    .join(" ");

    // Nav objects near the player and how much geometry they carry.
    let mut near_objects = 0;
    let mut near_cells = 0;
    let mut near_blocked_edges = 0;
    // How many of those blocked edges survive the storey-band filter that
    // `blocked_edge_crossed` applies — the difference between "there is a wall
    // here" and "the wall is even considered".
    let mut in_band = 0;
    // Outline edges and how many carry the `Global` flag, plus every distinct
    // flag byte seen. "No yellow lines on screen" only says this object has no
    // global edges; the histogram says whether the flag is used *at all*, which
    // is what decides whether it can carry the terrain hand-off.
    let mut near_outline_edges = 0;
    let mut near_global_edges = 0;
    let mut flag_bytes: Vec<u8> = Vec::new();
    let (mut nav_y_lo, mut nav_y_hi) = (f32::MAX, f32::MIN);
    // Which object the tracked location names, so "OnObject" identifies *what*.
    let mut standing_on = String::from("-");
    for (entity, object_nav, transform, name) in objects.iter() {
        let origin = transform.translation();
        if Vec2::new(origin.x, origin.z).distance(player_xz) > DIAGNOSTIC_RADIUS {
            continue;
        }
        near_objects += 1;
        if location.object() == Some(entity) {
            standing_on = name.map_or_else(|| format!("{entity}"), |n| n.to_string());
        }
        for handle in &object_nav.0 {
            let Some(bms) = bms_meshes.get(handle) else {
                continue;
            };
            let Some(nav_mesh) = &bms.navmesh else {
                continue;
            };
            near_cells += nav_mesh.cells.len();

            near_outline_edges += nav_mesh.outline_edges.len();
            for edge in nav_mesh
                .outline_edges
                .iter()
                .chain(nav_mesh.inline_edges.iter())
            {
                if edge.flag.is_global() {
                    near_global_edges += 1;
                }
                if !flag_bytes.contains(&edge.flag.0) {
                    flag_bytes.push(edge.flag.0);
                }
            }

            for edge in nav_mesh
                .outline_edges
                .iter()
                .chain(nav_mesh.inline_edges.iter())
                .filter(|edge| edge.flag.is_blocked())
            {
                near_blocked_edges += 1;
                let v0 = nav_mesh.vertices[edge.src_vertex as usize].position;
                let v1 = nav_mesh.vertices[edge.dst_vertex as usize].position;
                let edge_y =
                    0.5 * (transform.transform_point(v0).y + transform.transform_point(v1).y);
                nav_y_lo = nav_y_lo.min(edge_y);
                nav_y_hi = nav_y_hi.max(edge_y);
                if (edge_y - player.y).abs() <= OBJECT_BLOCK_Y_BAND {
                    in_band += 1;
                }
            }
        }
    }

    // Blocked terrain edges near the player, in the region covering them, plus
    // the two independent walkability signals at the player's own position.
    // Tile flag and cell openness are known to agree exactly in the data, so
    // disagreement here means the region-local position is being computed
    // wrong rather than the data being odd.
    let mut terrain_blocked_near = 0;
    let mut region_state = "MISSING";
    let mut here = String::from("local=? tile=? cell=?");
    for (nav_data, transform) in regions.iter() {
        let origin = transform.translation();
        let local = Vec2::new(origin.x - player.x, player.z - origin.z);
        if !(0.0..=REGION_SIZE).contains(&local.x) || !(0.0..=REGION_SIZE).contains(&local.y) {
            continue;
        }
        let Some(nav_mesh) = nav_meshes.get(&nav_data.0) else {
            region_state = "asset not loaded";
            continue;
        };
        region_state = "loaded";
        here = format!(
            "local=({:.0},{:.0}) tile_blocked={:?} cell={:?} cell_open={}",
            local.x,
            local.y,
            nav_mesh.tile_map.blocked_at(local),
            nav_mesh.quad_cell_list.cell_at(local),
            nav_mesh.quad_cell_list.is_walkable_at(local),
        );
        for (flag, line) in nav_mesh
            .global_edge_list
            .0
            .iter()
            .map(|e| (&e.flag, e.line))
            .chain(
                nav_mesh
                    .internal_edge_list
                    .0
                    .iter()
                    .map(|e| (&e.flag, e.line)),
            )
        {
            if flag.is_blocked() && ((line.0 + line.1) * 0.5).distance(local) <= DIAGNOSTIC_RADIUS {
                terrain_blocked_near += 1;
            }
        }
    }

    info!(
        "nav: loc={:?} pos=({:.0},{:.1},{:.0}) ground={:.1} | probes@{PROBE_STEP:.0}u: {} \
         on={} | objects<{DIAGNOSTIC_RADIUS:.0}: {} ({} cells, {} outline edges, {} global, \
         {} blocked edges, {} within Y band \
         {OBJECT_BLOCK_Y_BAND:.0}, edge Y {:.0}..{:.0}, flags {:?}) | terrain: {}, blocked edges<{DIAGNOSTIC_RADIUS:.0}: {} | {}",
        location,
        player.x,
        player.y,
        player.z,
        nav.surface_height(player_xz).unwrap_or(f32::NAN),
        probes,
        standing_on,
        near_objects,
        near_cells,
        near_outline_edges,
        near_global_edges,
        near_blocked_edges,
        in_band,
        if nav_y_lo == f32::MAX { 0.0 } else { nav_y_lo },
        if nav_y_hi == f32::MIN { 0.0 } else { nav_y_hi },
        flag_bytes,
        region_state,
        terrain_blocked_near,
        here,
    );
}

/// Marks where the cursor ray meets the nav mesh: green sphere on a walkable
/// cell, red on terrain outside every cell.
pub fn draw_nav_cursor_hit(
    settings: Res<RenderDebugSettings>,
    mut gizmos: Gizmos,
    cursor_cameras: Query<&GameCursorCamera>,
    nav_raycast: NavMeshRaycast,
) {
    if !settings.render_navmesh {
        return;
    }

    for cursor_camera in cursor_cameras.iter() {
        let Some(ray) = cursor_camera.cursor_ray else {
            continue;
        };
        let Some(hit) = nav_raycast.cast(&ray) else {
            continue;
        };
        let color = if hit.is_walkable() { GREEN } else { RED };
        gizmos.sphere(hit.point, 2.0, color);
    }
}

fn draw_nav_line(
    gizmos: &mut Gizmos,
    nav_mesh: &JMXVNVM,
    origin: Vec3,
    from: Vec2,
    to: Vec2,
    color: Color,
) {
    let steps = (from.distance(to) / SEGMENT_STEP).ceil().max(1.0) as usize;
    let mut prev = nav_point(nav_mesh, origin, from);
    for i in 1..=steps {
        let next = nav_point(nav_mesh, origin, from.lerp(to, i as f32 / steps as f32));
        gizmos.line(prev, next, color);
        prev = next;
    }
}

/// Region-local nav (x, z) → world position draped on the terrain. Regions are
/// spawned at `x * -REGION_SIZE` with X-mirrored meshes (see terrain/mod.rs), so
/// local X is negated relative to the region origin.
fn nav_point(nav_mesh: &JMXVNVM, origin: Vec3, p: Vec2) -> Vec3 {
    let height = nav_mesh.height_map.height_at(p.x, p.y);
    Vec3::new(
        origin.x - p.x,
        origin.y + height + LINE_LIFT,
        origin.z + p.y,
    )
}
