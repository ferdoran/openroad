//! Dev FPS graph: a tiny egui painter over a 10-second frame-by-frame history.
//! It samples the frame delta directly so the plot updates every frame, unlike
//! the cheaper text diagnostics panel that intentionally refreshes at 4 Hz.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

const HISTORY_SECS: f32 = 10.0;
const MIN_GRAPH_FPS: f32 = 30.0;
const GRAPH_SIZE: egui::Vec2 = egui::vec2(360.0, 150.0);
const GRAPH_MARGIN: f32 = 8.0;
const RECORDING_DURATIONS: [f32; 3] = [10.0, 30.0, 60.0];

pub(crate) struct FpsGraphPlugin;

impl Plugin for FpsGraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsGraphHistory>().add_systems(
            EguiPrimaryContextPass,
            fps_graph_window.run_if(super::dev_windows_visible),
        );
    }
}

#[derive(Clone, Copy, Debug)]
struct FpsSample {
    age_secs: f32,
    fps: f32,
}

#[derive(Resource, Default)]
struct FpsGraphHistory {
    samples: VecDeque<FpsSample>,
    recorder: FpsRecorder,
}

impl FpsGraphHistory {
    fn push_frame(&mut self, dt: Duration) {
        let dt_secs = dt.as_secs_f32();
        for sample in &mut self.samples {
            sample.age_secs += dt_secs;
        }
        while self
            .samples
            .front()
            .is_some_and(|sample| sample.age_secs > HISTORY_SECS)
        {
            self.samples.pop_front();
        }
        if dt_secs > f32::EPSILON {
            let sample = FpsSample {
                age_secs: 0.0,
                fps: 1.0 / dt_secs,
            };
            self.samples.push_back(sample);
            self.recorder.push_sample(sample.fps, dt_secs);
        }
    }

    fn current_fps(&self) -> Option<f32> {
        self.samples.back().map(|sample| sample.fps)
    }

    fn max_fps(&self) -> f32 {
        self.samples
            .iter()
            .map(|sample| sample.fps)
            .fold(MIN_GRAPH_FPS, f32::max)
            .ceil()
    }

    fn min_max_fps(&self) -> Option<(f32, f32)> {
        self.samples
            .iter()
            .map(|sample| sample.fps)
            .fold(None, |range, fps| match range {
                Some((min, max)) => Some((min.min(fps), max.max(fps))),
                None => Some((fps, fps)),
            })
    }
}

struct FpsRecorder {
    selected_secs: f32,
    active: Option<ActiveRecording>,
    completed: Option<CompletedRecording>,
}

impl Default for FpsRecorder {
    fn default() -> Self {
        Self {
            selected_secs: RECORDING_DURATIONS[0],
            active: None,
            completed: None,
        }
    }
}

impl FpsRecorder {
    fn selected_secs(&self) -> f32 {
        self.selected_secs
    }

    fn start(&mut self) {
        self.active = Some(ActiveRecording::new(self.selected_secs()));
        self.completed = None;
    }

    fn push_sample(&mut self, fps: f32, dt_secs: f32) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.push_sample(fps, dt_secs);
        if active.elapsed_secs >= active.target_secs {
            self.completed = active.finish();
            self.active = None;
        }
    }
}

struct ActiveRecording {
    target_secs: f32,
    elapsed_secs: f32,
    frames: u32,
    min_fps: f32,
    max_fps: f32,
}

impl ActiveRecording {
    fn new(target_secs: f32) -> Self {
        Self {
            target_secs,
            elapsed_secs: 0.0,
            frames: 0,
            min_fps: f32::INFINITY,
            max_fps: 0.0,
        }
    }

    fn push_sample(&mut self, fps: f32, dt_secs: f32) {
        self.elapsed_secs += dt_secs;
        self.frames += 1;
        self.min_fps = self.min_fps.min(fps);
        self.max_fps = self.max_fps.max(fps);
    }

    fn finish(&self) -> Option<CompletedRecording> {
        if self.frames == 0 || self.elapsed_secs <= f32::EPSILON {
            return None;
        }
        Some(CompletedRecording {
            target_secs: self.target_secs,
            elapsed_secs: self.elapsed_secs,
            average_fps: self.frames as f32 / self.elapsed_secs,
            min_fps: self.min_fps,
            max_fps: self.max_fps,
        })
    }
}

struct CompletedRecording {
    target_secs: f32,
    elapsed_secs: f32,
    average_fps: f32,
    min_fps: f32,
    max_fps: f32,
}

fn fps_graph_window(
    mut contexts: EguiContexts,
    time: Res<Time>,
    mut history: ResMut<FpsGraphHistory>,
) {
    history.push_frame(time.delta());

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::Window::new("FPS Graph")
        .default_pos(egui::pos2(16.0, 16.0))
        .default_size(egui::vec2(390.0, 220.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let fps = history.current_fps().unwrap_or(0.0);
                ui.label(egui::RichText::new(format!("{fps:.1} FPS")).strong());
                ui.separator();
                ui.label("last 10 seconds");
            });
            ui.add_space(4.0);
            draw_fps_graph(ui, &history);
            ui.add_space(4.0);
            if let Some((min, max)) = history.min_max_fps() {
                ui.label(format!("10s min/max: {min:.1} / {max:.1} FPS"));
            } else {
                ui.label("10s min/max: - / - FPS");
            }
            ui.add_space(6.0);
            recording_controls(ui, &mut history.recorder);
        });
}

fn recording_controls(ui: &mut egui::Ui, recorder: &mut FpsRecorder) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_label("Record for")
            .selected_text(format!("{:.0} seconds", recorder.selected_secs()))
            .show_ui(ui, |ui| {
                for seconds in RECORDING_DURATIONS {
                    ui.selectable_value(
                        &mut recorder.selected_secs,
                        seconds,
                        format!("{seconds:.0} seconds"),
                    );
                }
            });

        let label = if recorder.active.is_some() {
            "Recording..."
        } else {
            "Start"
        };
        if ui
            .add_enabled(recorder.active.is_none(), egui::Button::new(label))
            .clicked()
        {
            recorder.start();
        }
    });

    if let Some(active) = recorder.active.as_ref() {
        ui.label(format!(
            "Recording: {:.1} / {:.0}s",
            active.elapsed_secs.min(active.target_secs),
            active.target_secs
        ));
    }

    if let Some(completed) = recorder.completed.as_ref() {
        ui.label(format!(
            "Last {:.0}s recording ({:.1}s captured)",
            completed.target_secs, completed.elapsed_secs
        ));
        ui.label(format!("Average FPS: {:.1}", completed.average_fps));
        ui.label(format!("Min FPS: {:.1}", completed.min_fps));
        ui.label(format!("Max FPS: {:.1}", completed.max_fps));
    }
}

fn draw_fps_graph(ui: &mut egui::Ui, history: &FpsGraphHistory) {
    let (rect, _) = ui.allocate_exact_size(GRAPH_SIZE, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    let bg = egui::Color32::from_rgba_premultiplied(14, 17, 22, 220);
    let border = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(78));
    painter.rect_filled(rect, egui::CornerRadius::same(4), bg);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        border,
        egui::StrokeKind::Inside,
    );

    let graph = rect.shrink(GRAPH_MARGIN);
    let max_fps = history.max_fps();
    draw_grid(&painter, graph, max_fps);
    draw_line(&painter, graph, history, max_fps);
    draw_axis_labels(&painter, graph, max_fps);
}

fn draw_grid(painter: &egui::Painter, graph: egui::Rect, max_fps: f32) {
    let grid_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(43));
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let y = egui::lerp(graph.bottom()..=graph.top(), t);
        painter.line_segment(
            [egui::pos2(graph.left(), y), egui::pos2(graph.right(), y)],
            grid_stroke,
        );

        let x = egui::lerp(graph.left()..=graph.right(), t);
        painter.line_segment(
            [egui::pos2(x, graph.top()), egui::pos2(x, graph.bottom())],
            grid_stroke,
        );
    }

    let sixty = (60.0 / max_fps).clamp(0.0, 1.0);
    let y = egui::lerp(graph.bottom()..=graph.top(), sixty);
    painter.line_segment(
        [egui::pos2(graph.left(), y), egui::pos2(graph.right(), y)],
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(82, 74, 42)),
    );
}

fn draw_line(painter: &egui::Painter, graph: egui::Rect, history: &FpsGraphHistory, max_fps: f32) {
    if history.samples.len() < 2 {
        return;
    }

    let points: Vec<egui::Pos2> = history
        .samples
        .iter()
        .rev()
        .map(|sample| {
            let x_t = 1.0 - (sample.age_secs / HISTORY_SECS).clamp(0.0, 1.0);
            let y_t = (sample.fps / max_fps).clamp(0.0, 1.0);
            egui::pos2(
                egui::lerp(graph.left()..=graph.right(), x_t),
                egui::lerp(graph.bottom()..=graph.top(), y_t),
            )
        })
        .collect();

    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(100, 210, 255)),
    ));
}

fn draw_axis_labels(painter: &egui::Painter, graph: egui::Rect, max_fps: f32) {
    let font = egui::FontId::monospace(10.0);
    let color = egui::Color32::from_gray(180);
    painter.text(
        graph.left_top(),
        egui::Align2::LEFT_TOP,
        format!("{max_fps:.0} fps"),
        font.clone(),
        color,
    );
    painter.text(
        graph.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        "0 fps",
        font.clone(),
        color,
    );
    painter.text(
        graph.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        "now",
        font.clone(),
        color,
    );
    painter.text(
        graph.left_bottom() + egui::vec2(42.0, 0.0),
        egui::Align2::LEFT_BOTTOM,
        "-10s",
        font,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_only_the_last_ten_seconds() {
        let mut history = FpsGraphHistory::default();
        for _ in 0..12 {
            history.push_frame(Duration::from_secs(1));
        }

        assert!(history
            .samples
            .iter()
            .all(|sample| sample.age_secs <= HISTORY_SECS));
        assert_eq!(
            history.samples.back().map(|sample| sample.age_secs),
            Some(0.0)
        );
    }

    #[test]
    fn min_max_fps_uses_the_kept_history() {
        let mut history = FpsGraphHistory::default();
        history.push_frame(Duration::from_millis(100));
        history.push_frame(Duration::from_millis(50));

        assert_eq!(history.min_max_fps(), Some((10.0, 20.0)));
    }

    #[test]
    fn recorder_completes_with_total_metrics() {
        let mut recorder = FpsRecorder::default();
        recorder.selected_secs = 1.0;
        recorder.start();
        recorder.push_sample(10.0, 0.1);
        recorder.push_sample(20.0, 0.9);

        let completed = recorder.completed.as_ref().expect("recording completed");
        assert_eq!(completed.average_fps, 2.0);
        assert_eq!(completed.min_fps, 10.0);
        assert_eq!(completed.max_fps, 20.0);
    }
}
