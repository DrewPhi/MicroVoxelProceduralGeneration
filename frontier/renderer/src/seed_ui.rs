use egui::{Color32, Pos2, RichText, Sense, Vec2};
use frontier_world::{
    garden::Tissue,
    growth::{SeedProgram, Tile},
    Cell, VOXEL_SIZE,
};
use std::collections::BTreeMap;

#[derive(Clone)]
enum Drag {
    Add(Tile),
    Move(usize),
}

pub(crate) enum Action {
    Apply(SeedProgram),
    Close,
}

pub(crate) struct SeedEditor {
    draft: SeedProgram,
    active: char,
    path: Vec<usize>,
    selected: Option<usize>,
    target: char,
    round: usize,
    preview: Option<(BTreeMap<Cell, Tissue>, Vec<char>)>,
    text: Option<(String, String)>,
    error: Option<&'static str>,
}
impl SeedEditor {
    pub(crate) fn new(draft: SeedProgram) -> Self {
        let active = draft.start[0];
        Self {
            draft,
            active,
            path: Vec::new(),
            selected: None,
            target: active,
            round: 5,
            preview: None,
            text: None,
            error: None,
        }
    }
    fn select_rule(&mut self, name: char) {
        self.active = name;
        self.path.clear();
        self.selected = None;
    }
    fn changed(&mut self) {
        self.preview = None;
        self.text = None;
        self.error = None;
    }
    fn row(&mut self) -> &mut Vec<Tile> {
        let mut row = self
            .draft
            .rules
            .get_mut(&self.active)
            .expect("selected rule");
        for &index in &self.path {
            let Tile::Branch(inner) = &mut row[index] else {
                unreachable!("branch path")
            };
            row = inner;
        }
        row
    }
    fn drop_tile(&mut self, drag: Drag, index: usize) {
        let len = self.row().len();
        self.selected = match drag {
            Drag::Add(tile) if len < 256 && index <= len => {
                self.row().insert(index, tile);
                Some(index)
            }
            Drag::Move(from) if from < len && index <= len => {
                let tile = self.row().remove(from);
                let index = index.min(len - 1);
                self.row().insert(index, tile);
                Some(index)
            }
            _ => return,
        };
        self.changed();
    }
    fn remove_rule(&mut self) {
        let removed = self.active;
        self.draft.start.retain(|&tip| tip != removed);
        self.draft.rules.remove(&removed);
        fn strip(row: &mut Vec<Tile>, removed: char) {
            row.retain_mut(|tile| match tile {
                Tile::Continue(name) => *name != removed,
                Tile::Branch(inner) => {
                    strip(inner, removed);
                    !inner.is_empty()
                }
                _ => true,
            });
        }
        for row in self.draft.rules.values_mut() {
            strip(row, removed);
        }
        self.select_rule(self.draft.start[0]);
        self.target = self.active;
        self.changed();
    }
    fn tip(name: char) -> String {
        match name {
            'A' => "A · Starting shoot".into(),
            'T' => "T · Trunk".into(),
            'B' => "B · Branch".into(),
            c => format!("Tip {c}"),
        }
    }
    fn label(tile: &Tile) -> String {
        match tile {
            Tile::Grow(action, count) => {
                let name = match action {
                    'F' => "Grow forward",
                    'U' => "Grow up",
                    'L' => "Add leaves",
                    'V' => "Adaptive wood up",
                    _ => "Adaptive wood forward",
                };
                format!("{name} × {count}")
            }
            Tile::Turn(right) => if *right { "Turn right" } else { "Turn left" }.into(),
            Tile::Continue(tip) => format!("Continue as {tip}"),
            Tile::Branch(tiles) => match tiles.as_slice() {
                [Tile::Continue(tip)] => format!("Branch as {tip}"),
                _ => format!("Branch · {} tiles", tiles.len()),
            },
        }
    }
    fn preview(&mut self, ui: &mut egui::Ui) {
        ui.heading("Growth preview");
        ui.horizontal(|ui| {
            if ui.button("Restart").clicked() {
                self.round = 0;
                self.preview = None;
            }
            if ui
                .add_enabled(self.round < 24, egui::Button::new("Next round"))
                .clicked()
            {
                self.round += 1;
                self.preview = None;
            }
        });
        if ui
            .add(egui::Slider::new(&mut self.round, 0..=24).text("Round"))
            .changed()
        {
            self.preview = None;
        }
        let (cells, ran) = self
            .preview
            .get_or_insert_with(|| self.draft.preview(self.round));
        let top = cells.keys().map(|c| c.y).max().unwrap_or(0);
        ui.label(format!(
            "{:.2} m tall · {} voxels",
            (top + 1) as f64 * VOXEL_SIZE,
            cells.len()
        ));
        let tips: std::collections::BTreeSet<_> = ran.iter().copied().collect();
        ui.label(if tips.is_empty() {
            "No tips ran this round".into()
        } else {
            format!(
                "Tips this round  {}",
                tips.into_iter()
                    .map(|tip| tip.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let size = Vec2::new(ui.available_width().max(100.), 245.);
        let (rect, painter) = ui.allocate_painter(size, Sense::hover());
        painter.rect_filled(rect.rect, 6., Color32::from_rgb(40, 44, 38));
        let project = |x: f32, y: f32, z: f32| Vec2::new(x - z, (x + z) * 0.45 - y);
        let mut lo = Vec2::ZERO;
        let mut hi = Vec2::ZERO;
        for c in cells.keys() {
            let xy = project(c.x as f32, c.y as f32, c.z as f32);
            lo = lo.min(xy);
            hi = hi.max(xy);
        }
        let extent = hi - lo + Vec2::splat(4.);
        let scale = ((size.x - 18.) / extent.x)
            .min((size.y - 18.) / extent.y)
            .min(13.);
        let center = rect.rect.center() - (lo + hi) * 0.5 * scale;
        let point = |x, y, z| center + project(x, y, z) * scale;
        let mut ordered: Vec<_> = cells.iter().collect();
        ordered.sort_by_key(|(c, _)| 2 * (c.x + c.z) + c.y);
        for (&Cell { x, y, z }, &tissue) in ordered {
            let (x, y, z) = (x as f32, y as f32, z as f32);
            let a = point(x, y + 1., z);
            let b = point(x + 1., y + 1., z);
            let c = point(x + 1., y + 1., z + 1.);
            let d = point(x, y + 1., z + 1.);
            let rgb = match tissue {
                Tissue::Leaf => [130, 165, 91],
                Tissue::AdaptiveStem => [183, 133, 89],
                Tissue::Reinforcement => [209, 164, 113],
                _ => [143, 107, 75],
            };
            let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            let face = |vertices: [Pos2; 4], shade: f32| {
                painter.add(egui::Shape::convex_polygon(
                    vertices.to_vec(),
                    color.gamma_multiply(shade),
                    egui::Stroke::NONE,
                ));
            };
            let cell = |x: f32, y: f32, z: f32| Cell {
                x: x as i64,
                y: y as i64,
                z: z as i64,
            };
            if !cells.contains_key(&cell(x + 1., y, z)) {
                face([b, c, point(x + 1., y, z + 1.), point(x + 1., y, z)], 0.81);
            }
            if !cells.contains_key(&cell(x, y, z + 1.)) {
                face([d, c, point(x + 1., y, z + 1.), point(x, y, z + 1.)], 0.65);
            }
            if !cells.contains_key(&cell(x, y + 1., z)) {
                face([a, b, c, d], 1.);
            }
        }
    }

    fn rules(&mut self, ui: &mut egui::Ui) {
        let mut active = self.active;
        egui::ComboBox::from_id_salt("seed rule")
            .selected_text(Self::tip(active))
            .show_ui(ui, |ui| {
                for &name in self.draft.rules.keys() {
                    ui.selectable_value(&mut active, name, Self::tip(name));
                }
            });
        if active != self.active {
            self.select_rule(active);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.draft.available_name().is_some(),
                    egui::Button::new("Add rule"),
                )
                .clicked()
            {
                if let Some(name) = self.draft.available_name() {
                    self.draft.rules.insert(name, vec![Tile::Grow('U', 1)]);
                    self.select_rule(name);
                    self.changed();
                }
            }
            if ui
                .add_enabled(
                    self.draft.start.as_slice() != [self.active],
                    egui::Button::new("Start here"),
                )
                .clicked()
            {
                self.draft.start = vec![self.active];
                self.changed();
            }
            if ui
                .add_enabled(
                    self.draft.start.iter().any(|&t| t != self.active),
                    egui::Button::new("Remove rule"),
                )
                .clicked()
            {
                self.remove_rule();
            }
        });
        if !self.path.is_empty() && ui.button("Back to parent rule").clicked() {
            self.selected = self.path.pop();
        }
        let tiles = self.row().clone();
        let mut pending = None;
        egui::ScrollArea::vertical()
            .id_salt("seed tile row")
            .max_height(156.)
            .show(ui, |ui| {
                for (i, tile) in tiles.iter().enumerate() {
                    let response = ui.dnd_drag_source(ui.id().with(i), Drag::Move(i), |ui| {
                        ui.selectable_label(self.selected == Some(i), Self::label(tile))
                    });
                    if response.inner.clicked() {
                        self.selected = Some(i);
                    }
                    if let Some(drag) = response.response.dnd_release_payload::<Drag>() {
                        pending = Some(((*drag).clone(), i));
                    }
                }
                let end = ui.label("Drop tiles here");
                if let Some(drag) = end.dnd_release_payload::<Drag>() {
                    pending = Some(((*drag).clone(), tiles.len()));
                }
            });
        if let Some((drag, index)) = pending {
            self.drop_tile(drag, index);
        }
        if let Some(index) = self.selected {
            ui.horizontal_wrapped(|ui| {
                if let Some(Tile::Grow(_, count)) = self.row().get_mut(index) {
                    if ui
                        .add(egui::DragValue::new(count).range(1..=64).prefix("Repeat "))
                        .changed()
                    {
                        self.changed();
                    }
                }
                let len = self.row().len();
                if ui
                    .add_enabled(index > 0, egui::Button::new("Earlier"))
                    .clicked()
                {
                    self.drop_tile(Drag::Move(index), index - 1);
                }
                if ui
                    .add_enabled(index + 1 < len, egui::Button::new("Later"))
                    .clicked()
                {
                    self.drop_tile(Drag::Move(index), index + 1);
                }
                if matches!(self.row().get(index), Some(Tile::Branch(_)))
                    && ui.button("Edit branch").clicked()
                {
                    self.path.push(index);
                    self.selected = None;
                }
                if ui.button("Remove tile").clicked() && self.selected == Some(index) {
                    self.row().remove(index);
                    self.selected = None;
                    self.changed();
                }
            });
        }
        egui::ComboBox::from_id_salt("seed tip target")
            .selected_text(format!("Target tip {}", self.target))
            .show_ui(ui, |ui| {
                for &name in self.draft.rules.keys() {
                    ui.selectable_value(&mut self.target, name, Self::tip(name));
                }
            });
        let palette = [
            (Tile::Grow('U', 1), "Grow up"),
            (Tile::Grow('F', 1), "Forward"),
            (Tile::Grow('V', 1), "Adaptive up"),
            (Tile::Grow('W', 1), "Adaptive forward"),
            (Tile::Grow('L', 1), "Leaves"),
            (Tile::Turn(false), "Turn left"),
            (Tile::Turn(true), "Turn right"),
            (Tile::Continue(self.target), "Continue"),
            (Tile::Branch(vec![Tile::Continue(self.target)]), "Fork"),
            (Tile::Branch(Vec::new()), "Empty branch"),
        ];
        let per_row = if ui.available_width() >= 270. { 3 } else { 2 };
        egui::Grid::new("growth tile palette")
            .num_columns(per_row)
            .show(ui, |ui| {
                for (i, (tile, label)) in palette.into_iter().enumerate() {
                    let response = ui.dnd_drag_source(
                        ui.id().with(("palette", i)),
                        Drag::Add(tile.clone()),
                        |ui| ui.add(egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend)),
                    );
                    if response.inner.clicked() {
                        let end = self.row().len();
                        self.drop_tile(Drag::Add(tile), end);
                    }
                    if (i + 1) % per_row == 0 {
                        ui.end_row();
                    }
                }
            });
    }
    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let mut action = None;
        let width = (ctx.screen_rect().width() - 42.).clamp(180., 760.);
        egui::Window::new("Seed workshop")
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(width)
            .max_width(width)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                egui::ScrollArea::vertical()
                    .max_height((ctx.screen_rect().height() - 150.).max(180.))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            if ui.button("Starter seed").clicked() {
                                *self = Self::new(SeedProgram::default());
                            }
                            if ui.button("Full-size spruce").clicked() {
                                let program =
                                    SeedProgram::parse("A", include_str!("seed_spruce.txt"))
                                        .expect("valid spruce");
                                *self = Self::new(program);
                                self.round = 13;
                            }
                        });
                        if ui.available_width() >= 560. {
                            ui.columns(2, |columns| {
                                self.preview(&mut columns[0]);
                                self.rules(&mut columns[1]);
                            });
                        } else {
                            self.preview(ui);
                            self.rules(ui);
                        }
                        ui.collapsing("Rule text", |ui| {
                            let (start, source) =
                                self.text.get_or_insert_with(|| self.draft.source());
                            ui.label("Starting tips");
                            ui.text_edit_singleline(start);
                            ui.add(
                                egui::TextEdit::multiline(source)
                                    .desired_width((width - 30.).max(80.))
                                    .code_editor()
                                    .desired_rows(5),
                            );
                            if ui.button("Load text into tiles").clicked() {
                                match SeedProgram::parse(start, source) {
                                    Ok(program) => *self = Self::new(program),
                                    Err(error) => self.error = Some(error),
                                }
                            }
                        });
                    });
                if let Some(error) = self.error {
                    ui.colored_label(Color32::LIGHT_RED, RichText::new(error));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Apply to seed").clicked() {
                        let (start, source) = self.draft.source();
                        match SeedProgram::parse(&start, &source) {
                            Ok(program) => action = Some(Action::Apply(program)),
                            Err(error) => self.error = Some(error),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::Close);
                    }
                });
            });
        action
    }
}
