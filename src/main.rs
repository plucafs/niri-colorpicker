use eframe::egui;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

const MAX_COLORS: usize = 50;
const UI_ZOOM: f32 = 1.3;
const FLASH_DURATION: f32 = 0.45;
fn niri_focused_window_id() -> Option<u64> {
    let output = Command::new("niri")
        .args(["msg", "--json", "focused-window"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value.get("id")?.as_u64()
}

fn niri_focus_window(id: u64) -> bool {
    Command::new("niri")
        .args(["msg", "action", "focus-window", "--id", &id.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

// niri focuses the window under the cursor even while the pick-color grab is
// active, so put the focus back where it belongs after the pick.
fn restore_focus(want: Option<u64>, fallback: Option<u64>) {
    if let Some(id) = want {
        if niri_focused_window_id() == Some(id) {
            return;
        }
        if niri_focus_window(id) {
            return;
        }
    }
    if let Some(id) = fallback
        && Some(id) != want
    {
        let _ = niri_focus_window(id);
    }
}

trait ClipboardBackend {
    fn set_text(&mut self, text: &str) -> Result<(), String>;
}

struct WlCopyBackend;

impl ClipboardBackend for WlCopyBackend {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("wl-copy failed: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            let text = text.to_string();
            std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            });
        }
        Ok(())
    }
}

struct ArboardBackend {
    clipboard: arboard::Clipboard,
}

impl ArboardBackend {
    fn new() -> Result<Self, String> {
        let clipboard = arboard::Clipboard::new().map_err(|e| format!("{}", e))?;
        Ok(Self { clipboard })
    }
}

impl ClipboardBackend for ArboardBackend {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard
            .set_text(text.to_string())
            .map_err(|e| format!("{}", e))
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
    RgbModern,
    HslModern,
}

impl Default for ColorFormat {
    fn default() -> Self {
        Self::Hex
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct PaletteData {
    colors: Vec<[u8; 3]>,
    #[serde(default)]
    format: ColorFormat,
    #[serde(default = "default_dark_mode")]
    dark_mode: bool,
}

fn default_dark_mode() -> bool {
    true
}

impl Default for PaletteData {
    fn default() -> Self {
        Self {
            colors: Vec::new(),
            format: ColorFormat::default(),
            dark_mode: true,
        }
    }
}

fn format_color(color: [u8; 3], format: &ColorFormat) -> String {
    match format {
        ColorFormat::Hex => format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]),
        ColorFormat::Rgb => format!("rgb({}, {}, {})", color[0], color[1], color[2]),
        ColorFormat::Hsl => {
            let (h, s, l) = rgb_to_hsl(color[0], color[1], color[2]);
            format!("hsl({}, {}%, {}%)", h, s, l)
        }
        ColorFormat::RgbModern => format!("rgb({} {} {})", color[0], color[1], color[2]),
        ColorFormat::HslModern => {
            let (h, s, l) = rgb_to_hsl(color[0], color[1], color[2]);
            format!("hsl({} {}% {}%)", h, s, l)
        }
    }
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u32, u32, u32) {
    let r = r as f64 / 255.0;
    let g = g as f64 / 255.0;
    let b = b as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < f64::EPSILON {
        return (0, 0, (l * 100.0).round() as u32);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if max == r {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };

    (
        (h * 360.0).round() as u32,
        (s * 100.0).round() as u32,
        (l * 100.0).round() as u32,
    )
}

enum MenuAction {
    Copy,
    MoveTop,
    Edit,
    Delete,
}

fn draw_swatch(
    ui: &mut egui::Ui,
    id: egui::Id,
    color: [u8; 3],
    box_size: f32,
    flash: Option<f32>,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(box_size, box_size), egui::Sense::click());
    let response = ui.interact(rect, id, egui::Sense::click());

    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(color[0], color[1], color[2]));

    if response.hovered() {
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(2.0_f32, ui.visuals().hyperlink_color),
            egui::StrokeKind::Inside,
        );
    }

    if let Some(p) = flash {
        let alpha = (255.0 * p) as u8;
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(3.0_f32, egui::Color32::from_white_alpha(alpha)),
            egui::StrokeKind::Inside,
        );
    }

    response
}

struct App {
    palette: PaletteData,
    receiver: Option<mpsc::Receiver<String>>,
    clipboard: Box<dyn ClipboardBackend>,
    editing_color: Option<usize>,
    edit_gen: u32,
    context_menu: Option<(usize, egui::Pos2)>,
    flash: Option<(usize, Instant)>,
    ui_initialized: bool,
    startup_focus: Option<u64>,
}

impl App {
    fn new(startup_focus: Option<u64>) -> Self {
        let palette = Self::load_colors();
        Self {
            palette,
            receiver: None,
            clipboard: Box::new(WlCopyBackend),
            editing_color: None,
            edit_gen: 0,
            context_menu: None,
            flash: None,
            ui_initialized: false,
            startup_focus,
        }
    }

    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("niri-colorpicker").join("colors.json"))
    }

    fn load_colors() -> PaletteData {
        match Self::config_path() {
            Some(path) => {
                if let Ok(data) = fs::read_to_string(&path) {
                    serde_json::from_str(&data).unwrap_or_default()
                } else {
                    PaletteData::default()
                }
            }
            None => PaletteData::default(),
        }
    }

    fn save_colors(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(data) = serde_json::to_string_pretty(&self.palette) {
                let _ = fs::write(path, data);
            }
        }
    }

    fn pick_color(&mut self, ctx: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(true));

        let startup_focus = self.startup_focus;
        std::thread::spawn(move || {
            let focus_before = niri_focused_window_id();
            let mut ran = false;
            let result = match Command::new("niri").args(["msg", "pick-color"]).output() {
                Ok(output) => {
                    ran = output.status.success();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !output.status.success() {
                        format!("Error: {}", stderr.trim())
                    } else {
                        stdout.trim().to_string()
                    }
                }
                Err(e) => format!("Command error: {}", e),
            };
            if ran {
                restore_focus(startup_focus.or(focus_before), focus_before);
            }
            let _ = tx.send(result);
        });
    }

    fn parse_color(output: &str) -> Option<[u8; 3]> {
        let line = output.lines().find(|l| l.contains("rgb("))?;
        let start = line.find("rgb(")? + 4;
        let end = line.find(')')?;
        let coords: Vec<u8> = line[start..end]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        if coords.len() == 3 {
            Some([coords[0], coords[1], coords[2]])
        } else {
            None
        }
    }

    fn copy_color(&mut self, color: [u8; 3]) {
        if self.clipboard.set_text(&format_color(color, &self.palette.format)).is_ok() {
            self.flash = self
                .palette
                .colors
                .iter()
                .position(|&c| c == color)
                .map(|i| (i, Instant::now()));
        }
    }

    // After a color is removed from palette.colors at `removed`, the indexes
    // used by the editor and the flash get shifted by one above it.
    fn remap_after_removal(&mut self, removed: usize) {
        if let Some(e) = self.editing_color {
            if e == removed {
                self.editing_color = None;
            } else if e > removed {
                self.editing_color = Some(e - 1);
            }
        }
        self.flash = self.flash.and_then(|(i, at)| match i.cmp(&removed) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some((i - 1, at)),
            std::cmp::Ordering::Less => Some((i, at)),
        });
    }

    // After the color at `moved` is moved to the top, indexes before it shift
    // down by one and the color itself is at index 0.
    fn remap_after_move_to_top(&mut self, moved: usize) {
        if let Some(e) = self.editing_color {
            self.editing_color = if e == moved {
                Some(0)
            } else if e < moved {
                Some(e + 1)
            } else {
                Some(e)
            };
        }
        if let Some((i, at)) = self.flash {
            self.flash = Some((
                if i == moved {
                    0
                } else if i < moved {
                    i + 1
                } else {
                    i
                },
                at,
            ));
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.receiver.is_some() {
            // Picking: nothing is painted, the window must be fully see-through.
            [0.0, 0.0, 0.0, 0.0]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.ui_initialized {
            self.ui_initialized = true;
            ctx.set_zoom_factor(UI_ZOOM);
            ctx.set_theme(if self.palette.dark_mode {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
        }

        let received = match &self.receiver {
            Some(rx) => match rx.try_recv() {
                Ok(output) => Some(output),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some("Command error: picker stopped".into())
                }
            },
            None => None,
        };
        if let Some(output) = received {
            self.receiver = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(false));
            if output.starts_with("Error:") || output.starts_with("Command error:") {
                // The pick thread reports the failure; there is nothing to copy.
            } else if let Some(color) = Self::parse_color(&output) {
                self.copy_color(color);
                if let Some(pos) = self.palette.colors.iter().position(|&c| c == color) {
                    self.palette.colors.remove(pos);
                }
                self.palette.colors.insert(0, color);
                if self.palette.colors.len() > MAX_COLORS {
                    self.palette.colors.pop();
                }
                self.save_colors();
            }
        }

        if self.receiver.is_some() {
            ctx.request_repaint();
            return;
        }

        egui::TopBottomPanel::top("btn").show(ctx, |ui| {
            ui.add_space(8.0);
            if ui
                .add_sized([ui.available_width(), 40.0], egui::Button::new("Pick Color"))
                .clicked()
            {
                self.pick_color(ctx);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Format")
                    .width(ui.available_width() - 80.0)
                    .selected_text(format!("{:?}", self.palette.format))
                    .show_ui(ui, |ui| {
                        let formats = [
                            (ColorFormat::Hex, "HEX"),
                            (ColorFormat::Rgb, "RGB"),
                            (ColorFormat::Hsl, "HSL"),
                            (ColorFormat::RgbModern, "RGB Modern"),
                            (ColorFormat::HslModern, "HSL Modern"),
                        ];
                        for (fmt, label) in formats {
                            if ui
                                .selectable_value(&mut self.palette.format, fmt.clone(), label)
                                .clicked()
                            {
                                self.save_colors();
                            }
                        }
                    });
                let theme_label = if self.palette.dark_mode {
                    "☀"
                } else {
                    "🌙"
                };
                if ui.button(theme_label).clicked() {
                    self.palette.dark_mode = !self.palette.dark_mode;
                    ctx.set_theme(if self.palette.dark_mode {
                        egui::Theme::Dark
                    } else {
                        egui::Theme::Light
                    });
                    self.save_colors();
                }
            });
            ui.add_space(4.0);
        });

        let box_size = 28.0;
        let featured_size = 40.0;
        let gap = 8.0;

        let mut clicked_color: Option<[u8; 3]> = None;
        let mut rmb_index: Option<usize> = None;
        let mut rmb_pos: Option<egui::Pos2> = None;

        let flash_snapshot: Option<(usize, f32)> = self
            .flash
            .map(|(i, at)| (i, at.elapsed().as_secs_f32()))
            .filter(|(_, el)| *el < FLASH_DURATION);
        let flash_progress = |idx: usize| -> Option<f32> {
            flash_snapshot.as_ref().and_then(|(fi, el)| {
                if *fi == idx {
                    let p = (FLASH_DURATION - el) / FLASH_DURATION;
                    (p > 0.0).then_some(p)
                } else {
                    None
                }
            })
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            let colors = self.palette.colors.clone();
            if colors.is_empty() {
                return;
            }

            let labels: Vec<String> = colors
                .iter()
                .map(|color| format_color(*color, &self.palette.format))
                .collect();

            let small = ui
                .style()
                .text_styles
                .get(&egui::TextStyle::Small)
                .cloned()
                .unwrap_or(egui::FontId::new(9.0, egui::FontFamily::Proportional));
            let font_id = egui::FontId::new(small.size, egui::FontFamily::Monospace);
            let label_width = labels
                .iter()
                .map(|label| {
                    ui.fonts(|fonts| {
                        fonts
                            .layout_no_wrap(label.clone(), font_id.clone(), egui::Color32::WHITE)
                            .size()
                            .x
                    })
                })
                .fold(0.0_f32, f32::max);

            let text_color = ui.visuals().text_color();

            // Featured row: the most recent color, alone in its row above the rest.
            ui.horizontal(|ui| {
                let response = draw_swatch(
                    ui,
                    egui::Id::new(("color", 0usize)),
                    colors[0],
                    featured_size,
                    flash_progress(0),
                );
                if response.clicked() {
                    clicked_color = Some(colors[0]);
                }
                if response.secondary_clicked() {
                    rmb_index = Some(0);
                    rmb_pos = Some(
                        response
                            .interact_pointer_pos()
                            .unwrap_or_else(|| ui.cursor().min),
                    );
                }

                ui.label(
                    egui::RichText::new(&labels[0])
                        .monospace()
                        .small()
                        .color(text_color),
                );
            });

            ui.separator();

            if colors.len() > 1 {
                let cell_width = box_size + 6.0 + label_width;
                let columns = (((ui.available_width() + gap) / (cell_width + gap)).floor() as usize)
                    .clamp(1, MAX_COLORS);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("palette")
                            .num_columns(columns)
                            .spacing([gap, gap])
                            .show(ui, |ui| {
                                for (j, color) in colors[1..].iter().enumerate() {
                                    if j > 0 && j % columns == 0 {
                                        ui.end_row();
                                    }

                                    ui.horizontal(|ui| {
                                        let response = draw_swatch(
                                            ui,
                                            egui::Id::new(("color", j + 1)),
                                            *color,
                                            box_size,
                                            flash_progress(j + 1),
                                        );
                                        if response.clicked() {
                                            clicked_color = Some(*color);
                                        }
                                        if response.secondary_clicked() {
                                            rmb_index = Some(j + 1);
                                            rmb_pos = Some(
                                                response
                                                    .interact_pointer_pos()
                                                    .unwrap_or_else(|| ui.cursor().min),
                                            );
                                        }

                                        ui.label(
                                            egui::RichText::new(&labels[j + 1])
                                                .monospace()
                                                .small()
                                                .color(text_color),
                                        );
                                    });
                                }
                            });
                    });
            }
        });

        if let Some(color) = clicked_color {
            self.copy_color(color);
        }
        if let (Some(i), Some(pos)) = (rmb_index, rmb_pos) {
            // A right-click on a swatch opens its context menu at the pointer.
            self.context_menu = Some((i, pos));
        }

        // Colors vanished? Reset the context menu target.
        if let Some((idx, _)) = self.context_menu {
            if self.palette.colors.len() <= idx {
                self.context_menu = None;
            }
        }

        if let Some((idx, pos)) = self.context_menu {
            let color = self.palette.colors[idx];
            let mut menu_action: Option<MenuAction> = None;
            // A right-click this frame (re)opened the menu; don't let the
            // same click close it.
            let reopened = rmb_index.is_some();

            let area_response = egui::Area::new(egui::Id::new("color_context_menu"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(ctx, |ui| {
                    // Mirror egui's own menu styling (see egui::menu):
                    // no button chrome, only a hover fill, full-width rows.
                    {
                        let style = ui.style_mut();
                        style.spacing.button_padding = egui::vec2(2.0, 0.0);
                        style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
                        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                        style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                    }
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            ui.set_min_width(120.0);
                            if ui.button("Copy").clicked() {
                                menu_action = Some(MenuAction::Copy);
                            }
                            if idx > 0 && ui.button("Move to top").clicked() {
                                menu_action = Some(MenuAction::MoveTop);
                            }
                            if ui.button("Edit...").clicked() {
                                menu_action = Some(MenuAction::Edit);
                            }
                            if ui
                                .button(
                                    egui::RichText::new("Delete")
                                        .color(ui.visuals().error_fg_color),
                                )
                                .clicked()
                            {
                                menu_action = Some(MenuAction::Delete);
                            }
                        });
                    });
                });

            // Close when clicking anywhere outside the menu.
            let clicked_outside = ctx.input(|i| {
                i.pointer.any_click()
                    && !area_response
                        .response
                        .rect
                        .expand(4.0)
                        .contains(i.pointer.latest_pos().unwrap_or_default())
            });
            if clicked_outside && !reopened {
                self.context_menu = None;
            }

            if let Some(action) = menu_action {
                match action {
                    MenuAction::Copy => {
                        self.copy_color(color);
                    }
                    MenuAction::MoveTop => {
                        let selected = self.palette.colors.remove(idx);
                        self.palette.colors.insert(0, selected);
                        self.remap_after_move_to_top(idx);
                        self.save_colors();
                    }
                    MenuAction::Edit => {
                        // New generation id per open: makes `default_pos`
                        // re-center the window each time it is opened.
                        self.edit_gen += 1;
                        self.editing_color = Some(idx);
                    }
                    MenuAction::Delete => {
                        self.palette.colors.remove(idx);
                        self.remap_after_removal(idx);
                        self.save_colors();
                    }
                }
                self.context_menu = None;
            }
        }

        if let Some(idx) = self.editing_color {
            let mut open = true;
            egui::Window::new("Edit Color")
                .id(egui::Id::new(("edit_color", self.edit_gen)))
                .pivot(egui::Align2::CENTER_CENTER)
                .default_pos(ctx.screen_rect().center())
                .resizable(false)
                .collapsible(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    let mut new_color = *self.palette.colors.get(idx).unwrap_or(&[0; 3]);
                    ui.label("Pick a new color:");
                    let response =
                        egui::color_picker::color_edit_button_srgb(ui, &mut new_color);
                    if response.changed() {
                        if let Some(slot) = self.palette.colors.get_mut(idx) {
                            *slot = new_color;
                        }
                        self.save_colors();
                        self.copy_color(new_color);
                    }
                    ui.add_space(4.0);
                    if ui.button("Done").clicked() {
                        self.editing_color = None;
                    }
                });
            if !open {
                self.editing_color = None;
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    // Captured before our own window maps: this is the window the user was
    // working in, and the focus must go back there after a pick.
    let startup_focus = niri_focused_window_id();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 500.0])
            .with_min_inner_size([400.0, 500.0])
            .with_max_inner_size([400.0, 500.0])
            .with_resizable(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_title("Niri Color Picker"),
        ..Default::default()
    };

    eframe::run_native(
        "Niri Color Picker",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(startup_focus)))),
    )
}
