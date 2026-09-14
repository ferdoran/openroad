//! Idea: an offline preview of every party surface at once — the roster page,
//! the quick-party board, the mode modal and the matching board with its
//! dialogs — driven by a mock roster and a mock match page. Run with
//! `SCENE=ui_testing`.
//!
//! This exists because the party feature is the one surface that cannot be
//! inspected alone: it needs a *second player*, and half of it only appears
//! once a server has pushed a roster. Seeding the same resources the network
//! layer writes gives the whole thing a look without a login, and it exercises
//! the presence-mask fold rather than sidestepping it — every mock record
//! carries a real mask, and one of them is deliberately partial so the "unknown
//! field draws empty" path is on screen rather than only in a test.
//!
//! The windows' own Update systems already run in `SceneState::UiTesting`, so
//! this only spawns them, seeds the state and opens them.

use bevy::prelude::*;

use packets::agent::party::{
    PartyData, PartyMatchEntry, PartyMemberCore, PartyMemberMask, PartyPositionWorld, PartySetup,
    PARTY_PURPOSE_HUNTING, PARTY_PURPOSE_THIEF, PARTY_PURPOSE_TRADER,
};

use crate::plugins::hud::party::model::PartyWindowState;
use crate::plugins::hud::party::ui::spawn_party_window;
use crate::plugins::hud::party_matching::model::PartyMatchState;
use crate::plugins::hud::party_matching::ui::spawn_match_window;
use crate::plugins::hud::quick_party::spawn_quick_party_board;
use crate::plugins::net::party::PartyRoster;
use crate::scenes::SceneState;

pub struct PartyUiPreviewPlugin;

impl Plugin for PartyUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (
                seed_mock_party,
                spawn_party_window,
                spawn_quick_party_board,
                spawn_match_window,
            )
                .chain(),
        );
    }
}

/// A full-mask member — what a roster push sends.
fn member(
    id: u32,
    name: &str,
    level: u8,
    hp_mp: u8,
    guild: &str,
    masteries: (u32, u32),
) -> PartyMemberCore {
    PartyMemberCore {
        presence: PartyMemberMask::ALL,
        member_id: Some(id),
        name: Some(name.to_string()),
        // 1907 is `CHAR_CH_MAN_ADVENTURER`, a real characterdata row, so the
        // portrait and the kindred mark resolve against actual data.
        model_id: Some(1907),
        level: Some(level),
        hp_mp: Some(hp_mp),
        region: Some(25000),
        position_dungeon: None,
        position_world: Some(PartyPositionWorld {
            x: 800,
            y: 0,
            z: 900,
        }),
        position_tail: Some(0),
        guild_name: Some(guild.to_string()),
        flag: Some(0),
        mastery_primary: Some(masteries.0),
        mastery_secondary: Some(masteries.1),
    }
}

fn seed_mock_party(
    mut roster: ResMut<PartyRoster>,
    mut party: ResMut<PartyWindowState>,
    mut matching: ResMut<PartyMatchState>,
) {
    // 257/259 are real China mastery ids, so the badges resolve to real icons.
    let mut members = vec![
        member(101, "Yuwen", 62, 0xAA, "IronLotus", (257, 259)),
        member(102, "Baekho", 58, 0x76, "IronLotus", (257, 0)),
        member(103, "Solveig", 71, 0x5A, "", (259, 257)),
        member(104, "Tarik", 44, 0x33, "Sandstorm", (257, 259)),
    ];
    // ...and one member the server has only partially described. Its level,
    // guild and portrait must draw EMPTY rather than as zeroes — this is the
    // presence-mask path, on screen.
    members.push(PartyMemberCore {
        presence: PartyMemberMask::MEMBER_ID | PartyMemberMask::NAME | PartyMemberMask::HP_MP,
        member_id: Some(105),
        name: Some("Halfknown".to_string()),
        model_id: Some(1907),
        hp_mp: Some(0x28),
        ..Default::default()
    });

    roster.apply_data(&PartyData {
        presence: 0x03,
        party_number: 4242,
        master_join_id: Some(103),
        setup: Some(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED),
        member_count: Some(members.len() as u8),
        members,
    });
    party.open = true;

    matching.entries = vec![
        match_entry(
            1,
            "Solveig",
            "Uigur run, need blader",
            60,
            80,
            4,
            PARTY_PURPOSE_HUNTING,
        ),
        match_entry(
            2,
            "Tarik",
            "Trade run to Hotan",
            40,
            70,
            6,
            PARTY_PURPOSE_TRADER,
        ),
        match_entry(
            3,
            "Nasir",
            "Thief hunting, jangan",
            50,
            90,
            2,
            PARTY_PURPOSE_THIEF,
        ),
        match_entry(
            4,
            "Baekho",
            "Karakoram farm",
            70,
            100,
            7,
            PARTY_PURPOSE_HUNTING,
        ),
    ];
    matching.page_index = 0;
    matching.page_count = 3;
    matching.open = true;
}

fn match_entry(
    number: u32,
    master: &str,
    title: &str,
    level_min: u8,
    level_max: u8,
    members: u8,
    purpose: u8,
) -> PartyMatchEntry {
    PartyMatchEntry {
        number,
        registered_at: 0,
        master_name: master.to_string(),
        race_type: 0,
        member_count: members,
        setup: PartySetup::EXP_SHARED,
        purpose,
        level_min,
        level_max,
        title: title.to_string(),
    }
}
