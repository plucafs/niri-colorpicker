use eframe::egui;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;

const MAX_COLORS: usize = 10;

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

#[derive(Serialize, Deserialize, Clone)]
struct PaletteData {
    colors: Vec<[u8; 3]>,
}

impl Default for PaletteData {
    fn default() -> Self {
        Self {
            colors: Vec::new(),
        }
    }
}

struct App {
    palette: PaletteData,
    status: String,
    receiver: Option<mpsc::Receiver<String>>,
    clipboard: Box<dyn ClipboardBackend>,
}

impl App {
    fn new() -> Self {
        let palette = Self::load_colors();
        Self {
            palette,
            status: String::new(),
            receiver: None,
            clipboard: Box::new(WlCopyBackend),
        }
    }

    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("niri-color-picker").join("colors.json"))
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
        self.status = "Picking...".into();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));

        std::thread::spawn(move || {
            let result = match Command::new("niri")
                .args(["msg", "pick-color"])
                .output()
            {
                Ok(output) => {
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

    fn to_hex(color: [u8; 3]) -> Option<String> {
        Some(format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]))
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.receiver {
            if let Ok(output) = rx.try_recv() {
                self.receiver = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                if output.starts_with("Error:") || output.starts_with("Command error:") {
                    self.status = output;
                } else if let Some(color) = Self::parse_color(&output) {
                    if let Some(hex) = Self::to_hex(color) {
                                    match self.clipboard.set_text(&hex) {
                            Ok(_) => self.status = format!("Copied: {}", hex),
                            Err(e) => self.status = format!("Clipboard error: {}", e),
                        }
                        self.palette.colors.insert(0, color);
                        if self.palette.colors.len() > MAX_COLORS {
                            self.palette.colors.pop();
                        }
                        self.save_colors();
                    }
                } else {
                    self.status = format!("Parse error: {}", output);
                }
            } else {
                ctx.request_repaint();
            }
        }

        egui::TopBottomPanel::top("btn").show(ctx, |ui| {
            ui.add_space(8.0);
            if ui
                .add_sized([ui.available_width(), 40.0], egui::Button::new("Pick Color"))
                .clicked()
            {
                self.pick_color(ctx);
            }
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            if !self.status.is_empty() {
                ui.label(egui::RichText::new(&self.status).small().weak());
            }
            ui.add_space(4.0);
        });

        let box_size = 28.0;
        let spacing = 4.0;

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        for (i, color) in self.palette.colors.iter().enumerate() {
                            let color32 = egui::Color32::from_rgb(color[0], color[1], color[2]);
                            let hex = Self::to_hex(*color).unwrap_or_default();

                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(box_size, box_size),
                                    egui::Sense::click(),
                                );

                                let response = ui.interact(
                                    rect,
                                    egui::Id::new(("color", i)),
                                    egui::Sense::click(),
                                );

                                let painter = ui.painter();
                                painter.rect_filled(rect, 4.0, color32);

                                if response.hovered() {
                                    painter.rect_stroke(
                                        rect,
                                        4.0,
                                        egui::Stroke::new(
                                            2.0_f32,
                                            egui::Color32::WHITE,
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                }

                                if response.clicked() {
                        match self.clipboard.set_text(&hex) {
                                        Ok(_) => self.status = format!("Copied: {}", hex),
                                        Err(e) => self.status = format!("Clipboard error: {}", e),
                                    }
                                }

                                ui.label(
                                    egui::RichText::new(&hex).monospace().small(),
                                );
                            });

                            ui.add_space(spacing);
                        }
                    });
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([200.0, 300.0])
            .with_min_inner_size([200.0, 300.0])
            .with_max_inner_size([200.0, 300.0])
            .with_resizable(false)
            .with_title("Niri Color Picker"),
        ..Default::default()
    };

    eframe::run_native(
        "Niri Color Picker",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
