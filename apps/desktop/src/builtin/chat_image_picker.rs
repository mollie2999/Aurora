use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use egui::{Align, Id, Key, Layout, Ui, vec2};
use i18n::t;
use ui::{Theme, icons, widgets as w};

pub(super) const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Clone, Default)]
pub(super) struct ImagePicker {
    directory: PathBuf,
    address: String,
    entries: Vec<(bool, PathBuf)>,
    error: Option<String>,
}

impl ImagePicker {
    pub(super) fn new() -> Self {
        let directory = directories::UserDirs::new()
            .map(|dirs| {
                dirs.picture_dir()
                    .filter(|path| path.is_dir())
                    .unwrap_or_else(|| dirs.home_dir())
                    .to_path_buf()
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut picker = Self::default();
        picker.address = directory.to_string_lossy().into_owned();
        picker.open(directory);
        picker
    }

    fn open(&mut self, directory: PathBuf) {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        self.address = directory.to_string_lossy().into_owned();
        self.directory = directory;
        self.error = None;
        self.entries.clear();
        self.entries.extend(entries.flatten().filter_map(|entry| {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            let is_dir = file_type.is_dir() || (file_type.is_symlink() && path.is_dir());
            (is_dir || (is_image(&path) && (file_type.is_file() || path.is_file())))
                .then_some((!is_dir, path))
        }));
        self.entries.sort_unstable();
    }

    pub(super) fn show(mut self, ui: &mut Ui, t: &Theme, id: Id) -> Option<PathBuf> {
        let ctx = ui.ctx();
        let size = ctx.content_rect().size();
        let modal = w::modal(ctx, id).show(ctx, |ui| {
            ui.set_width((size.x - 72.0).clamp(160.0, 460.0));
            ui.label(w::text_bold(t!("Upload an image"), 18.0, t.text));
            let destination = ui
                .horizontal(|ui| {
                    let parent = self
                        .directory
                        .parent()
                        .filter(|path| !path.as_os_str().is_empty());
                    let up = ui
                        .add_enabled_ui(parent.is_some(), |ui| {
                            w::icon_button(ui, t, icons::CARET_UP, t!("Parent folder"), false, 0)
                                .clicked()
                        })
                        .inner;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let open = w::button(ui, t, None, t!("Open"), w::Kind::Ghost, w::Size::Sm)
                            .clicked();
                        let address = ui.add(
                            egui::TextEdit::singleline(&mut self.address)
                                .hint_text(t!("Folder or image path"))
                                .desired_width(ui.available_width()),
                        );
                        if up {
                            parent.map(Path::to_path_buf)
                        } else if open
                            || (address.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                        {
                            Some(self.directory.join(self.address.trim()))
                        } else {
                            None
                        }
                    })
                    .inner
                })
                .inner;
            if let Some(path) = destination {
                if is_image(&path) && path.is_file() {
                    return Some(path);
                }
                self.open(path);
            }
            if let Some(error) = &self.error {
                ui.label(w::text(error, 13.0, t.text_dim));
            } else if self.entries.is_empty() {
                ui.label(w::text(t!("No images or folders"), 13.0, t.text_dim));
            }

            let selected = egui::ScrollArea::vertical()
                .id_salt(&self.directory)
                .max_height((size.y - 180.0).clamp(60.0, 280.0))
                .show_rows(ui, 28.0, self.entries.len(), |ui, rows| {
                    let mut selected = None;
                    for (is_file, path) in &self.entries[rows] {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let icon = if *is_file {
                            icons::IMAGE
                        } else {
                            icons::FOLDER
                        };
                        let label = w::text(format!("{icon}  {name}"), 13.0, t.text);
                        if ui
                            .add(
                                egui::Button::new(label)
                                    .frame(false)
                                    .truncate()
                                    .min_size(vec2(0.0, 28.0)),
                            )
                            .on_hover_text(name.as_ref())
                            .clicked()
                        {
                            selected = Some((*is_file, path.clone()));
                        }
                    }
                    selected
                })
                .inner;
            if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                ui.close();
            }
            match selected {
                Some((true, path)) => Some(path),
                Some((false, path)) => {
                    self.open(path);
                    ctx.request_repaint();
                    None
                }
                None => None,
            }
        });
        if modal.inner.is_none() && !modal.should_close() {
            ctx.data_mut(|data| data.insert_temp(id, self));
        }
        modal.inner
    }
}

fn is_image(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        IMAGE_EXTENSIONS
            .iter()
            .any(|supported| supported.eq_ignore_ascii_case(ext))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_closes_the_picker_and_is_consumed() {
        let ctx = egui::Context::default();
        let t = Theme::default();
        let id = Id::unique("test-chat-image-picker");
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(ImagePicker::default().show(ui, &t, id).is_none());
        });
        output.textures_delta.clear();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let picker = ui
                .data_mut(|data| data.remove_temp::<ImagePicker>(id))
                .unwrap();
            assert!(picker.show(ui, &t, id).is_none());
            assert!(!ui.input(|input| input.key_pressed(Key::Escape)));
            assert!(ui.data(|data| data.get_temp::<ImagePicker>(id)).is_none());
        });
        output.textures_delta.clear();
    }
}
