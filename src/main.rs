//! `fbm`: deliberately small, button-driven terrain authoring utility.
mod preview;
mod terrain;
use eframe::egui;
use preview::{Picture, Recipe, View};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

struct Completed {
    id: u64,
    result: Result<(Option<Picture>, Option<Picture>, Recipe, PathBuf, f64), String>,
}
struct App {
    recipe: Recipe,
    shown: Option<Recipe>,
    texture: Option<egui::TextureHandle>,
    caption: String,
    overview: Option<(egui::TextureHandle, terrain::Params)>,
    output: String,
    recipe_path: String,
    status: String,
    token: Arc<AtomicU64>,
    tx: mpsc::Sender<Completed>,
    rx: mpsc::Receiver<Completed>,
    busy: bool,
}
impl App {
    fn new(recipe: Recipe) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            recipe,
            shown: None,
            texture: None,
            caption: String::new(),
            overview: None,
            output: "output/fbm".into(),
            recipe_path: "output/fbm/recipe.json".into(),
            status: "Adjust settings, then press Preview. No automatic regeneration.".into(),
            token: Arc::new(AtomicU64::new(0)),
            tx,
            rx,
            busy: false,
        }
    }
    fn start(&mut self, ctx: &egui::Context, export: bool) {
        if let Err(e) = self.recipe.validate() {
            self.status = e;
            return;
        }
        let id = self.token.fetch_add(1, Ordering::Relaxed) + 1;
        let recipe = self.recipe.clone();
        let need_map = !export
            && recipe.view != View::Overview
            && self
                .overview
                .as_ref()
                .is_none_or(|(_, p)| *p != recipe.terrain);
        let token = self.token.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let root = PathBuf::from(&self.output);
        self.busy = true;
        self.status = if export {
            "Exporting 16 m occupancy fixture…"
        } else {
            "Building preview…"
        }
        .into();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = (|| {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos();
                let dir = root.join(format!("{stamp}-{id}"));
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let picture = if export {
                    preview::export_voxels(&recipe, &dir.join("region.fbmvox"), &token, id)?;
                    None
                } else {
                    let p = preview::render(&recipe, token.clone(), id)?;
                    if token.load(Ordering::Relaxed) != id {
                        return Err("Cancelled".into());
                    }
                    preview::save_png(&p, &dir.join("preview.png"))?;
                    Some(p)
                };
                let overview = if need_map {
                    let mut map_recipe = recipe.clone();
                    map_recipe.view = View::Overview;
                    map_recipe.width = 256;
                    Some(preview::render(&map_recipe, token.clone(), id)?)
                } else {
                    None
                };
                save_recipe(&recipe, &dir.join("recipe.json"))?;
                Ok((
                    picture,
                    overview,
                    recipe,
                    dir,
                    start.elapsed().as_secs_f64(),
                ))
            })();
            let _ = tx.send(Completed { id, result });
            ctx.request_repaint();
        });
    }
}
fn number(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).range(range).speed(speed));
    });
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(done) = self.rx.try_recv() {
            if done.id != self.token.load(Ordering::Relaxed) {
                continue;
            }
            self.busy = false;
            match done.result {
                Ok((picture, overview, recipe, dir, seconds)) => {
                    if let Some(p) = overview {
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                        self.overview = Some((
                            ctx.load_texture("world locator", image, egui::TextureOptions::LINEAR),
                            recipe.terrain.clone(),
                        ));
                    }
                    if let Some(p) = picture {
                        self.caption = p.description;
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                        self.texture = Some(ctx.load_texture(
                            "terrain preview",
                            image,
                            egui::TextureOptions::NEAREST,
                        ));
                        if recipe.view == View::Overview {
                            self.overview = Some((
                                self.texture.as_ref().unwrap().clone(),
                                recipe.terrain.clone(),
                            ));
                        }
                        self.shown = Some(recipe);
                    }
                    self.status = format!("Saved {} · {:.2} s", dir.display(), seconds);
                }
                Err(e) => self.status = e,
            }
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
        });
        egui::SidePanel::left("controls").default_width(300.).show(ctx,|ui|{
      egui::ScrollArea::vertical().show(ui,|ui|{
        ui.heading("fbm terrain builder");ui.label("Generate target terrain for analytic fitting.");
        ui.separator();
        ui.horizontal(|ui|{ui.label("Seed");ui.add(egui::DragValue::new(&mut self.recipe.terrain.seed));});
        let p=&mut self.recipe.terrain;
        number(ui,"World edge (m)",&mut p.world_m,16. ..=100_000.,100.);
        number(ui,"Vertical extent (m)",&mut p.ceiling_m,16. ..=4096.,16.);
        number(ui,"Voxel edge (cm)",&mut p.voxel_cm,0.5..=200.,0.25);
        ui.separator();ui.strong("Landforms");
        number(ui,"Base elevation (m)",&mut p.base_m,0. ..=p.ceiling_m,1.);
        number(ui,"Relief (m)",&mut p.relief_m,0. ..=2000.,1.);
        number(ui,"Base wavelength (m)",&mut p.wavelength_m,1. ..=100_000.,10.);
        ui.horizontal(|ui|{ui.label("Octaves");ui.add(egui::DragValue::new(&mut p.octaves).range(1..=12));});
        number(ui,"Gain",&mut p.gain,0. ..=0.9,0.01);
        number(ui,"Lacunarity",&mut p.lacunarity,1.1..=4.,0.05);
        ui.strong("3D surface and caves");
        number(ui,"3D roughness (m)",&mut p.overhang_m,0. ..=100.,0.5);
        number(ui,"Rock wavelength (m)",&mut p.rock_scale_m,1. ..=500.,1.);
        number(ui,"Cave wavelength (m)",&mut p.cave_scale_m,1. ..=1000.,1.);
        number(ui,"Cave band width",&mut p.cave_width,0. ..=0.7,0.01);
        number(ui,"Cave roof cover (m)",&mut p.cave_roof_m,0. ..=100.,1.);
        ui.separator();ui.strong("Inspection");
        let i=&mut self.recipe.inspection;
        i.x=i.x.clamp(0.,(p.world_m-p.voxel_m()).max(0.));i.z=i.z.clamp(0.,(p.world_m-p.voxel_m()).max(0.));
        number(ui,"Center X (m)",&mut i.x,0. ..=p.world_m-p.voxel_m(),1.);
        number(ui,"Center Z (m)",&mut i.z,0. ..=p.world_m-p.voxel_m(),1.);
        number(ui,"Local width (m)",&mut i.span,4. ..=256.,1.);
        number(ui,"Orbit yaw (degrees)",&mut i.yaw,-360. ..=360.,2.);
        if self.recipe.view == View::Isometric { ui.label("Isometric elevation: 35.26° (fixed)"); }
        else { number(ui,"Orbit elevation",&mut i.elevation,5. ..=85.,1.); }
        number(ui,"Slice depth (m)",&mut i.slice_depth,1. ..=1000.,1.);
        ui.horizontal_wrapped(|ui|{
          ui.selectable_value(&mut self.recipe.view,View::Overview,"Overview");
          ui.selectable_value(&mut self.recipe.view,View::Local,"Local voxels");
          ui.selectable_value(&mut self.recipe.view,View::Isometric,"Isometric");
          ui.selectable_value(&mut self.recipe.view,View::Slice,"Cutaway");
        });
        ui.horizontal(|ui|{ui.label("Image width (px)");ui.add(egui::DragValue::new(&mut self.recipe.width).range(64..=1024).speed(16.));});
        ui.label("Local views use exact cell occupancy. Higher resolution and larger inspection regions take longer.");
        if ui.button("Preview").clicked(){self.start(ctx,false);}
        if self.busy && ui.button("Cancel").clicked(){self.token.fetch_add(1,Ordering::Relaxed);self.busy=false;self.status="Cancelled; previous preview retained".into();}
        if ui.button("Export 16 m voxel fixture").clicked(){self.start(ctx,true);}
        ui.label("Fixture: native occupancy near selected location; maximum 32 million cells. No whole-world voxel allocation.");
        ui.separator();ui.label("Output directory");ui.text_edit_singleline(&mut self.output);
        ui.label("Recipe file (JSON)");ui.text_edit_singleline(&mut self.recipe_path);
        ui.horizontal(|ui|{
          if ui.button("Save settings").clicked(){self.status=match save_recipe(&self.recipe,Path::new(&self.recipe_path)){Ok(())=>"Settings saved".into(),Err(e)=>e};}
          if ui.button("Load settings").clicked(){match load_recipe(Path::new(&self.recipe_path)){Ok(r)=>{self.recipe=r;self.status="Settings loaded; press Preview".into();},Err(e)=>self.status=e}}
        });
        ui.separator();ui.label(&self.status);
      });
    });
        egui::SidePanel::right("world_locator").resizable(false).default_width(210.).show(ctx,|ui| {
            ui.heading("World locator");
            if let Some((texture,params))=&self.overview {
                let size=egui::vec2(ui.available_width(),ui.available_width());
                let response=ui.add(egui::Image::new((texture.id(),size)));
                let footprint=self.shown.as_ref().filter(|r|r.view!=View::Overview && r.terrain==*params).unwrap_or(&self.recipe);
                draw_footprint(ui,response.rect,footprint,params.world_m);
                ui.label("North ↑ · East →");
                ui.label("Red box: inspected footprint");
                ui.small("Crosshair locates chunks smaller than a few map pixels. Box remains to scale.");
                if *params!=self.recipe.terrain {ui.colored_label(egui::Color32::YELLOW,"Previous terrain map; Preview refreshes it.");}
            } else {ui.label("Press Preview to generate the locator map.");}
        });
        egui::CentralPanel::default().show(ctx,|ui|{
        ui.heading("Terrain inspection");
        if self.recipe.view != View::Overview {
            let mut pan = [0.,0.];
            let i=&self.recipe.inspection;
            let edge=self.recipe.terrain.world_m-self.recipe.terrain.voxel_m();
            egui::Grid::new("chunk_navigation").show(ui,|ui| {
                ui.label("");
                if ui.add_enabled(i.z>0.,egui::Button::new("↑ North")).on_hover_text("Move one chunk toward −Z and preview").clicked(){pan[1]=-1.;}
                ui.end_row();
                if ui.add_enabled(i.x>0.,egui::Button::new("← West")).on_hover_text("Move one chunk toward −X and preview").clicked(){pan[0]=-1.;}
                ui.label(format!("{:.1} m step",i.span));
                if ui.add_enabled(i.x<edge,egui::Button::new("East →")).on_hover_text("Move one chunk toward +X and preview").clicked(){pan[0]=1.;}
                ui.end_row();
                ui.label("");
                if ui.add_enabled(i.z<edge,egui::Button::new("↓ South")).on_hover_text("Move one chunk toward +Z and preview").clicked(){pan[1]=1.;}
                ui.end_row();
            });
            ui.label(format!("Inspection center: X {:.2} m, Z {:.2} m",i.x,i.z));
            if pan != [0.,0.] {
                let i=&mut self.recipe.inspection;
                i.x=(i.x+pan[0]*i.span).clamp(0.,edge);
                i.z=(i.z+pan[1]*i.span).clamp(0.,edge);
                self.start(ctx,false);
            }
        }

        if let Some(shown)=&self.shown {
            if shown.terrain!=self.recipe.terrain || shown.inspection!=self.recipe.inspection || shown.view!=self.recipe.view || shown.width!=self.recipe.width {
                ui.colored_label(egui::Color32::YELLOW,"Settings changed. Displaying previous result until Preview completes.");
            }
        }
        ui.label(&self.caption);
        if let Some(texture)=&self.texture {
            let size=texture.size_vec2();let available=ui.available_size();let scale=(available.x/size.x).min(available.y/size.y).min(2.).max(0.1);
            let response=ui.add(egui::Image::new((texture.id(),size*scale)).sense(egui::Sense::click()));
            if let Some(shown)=&self.shown {
                if shown.view==View::Overview {
                    draw_footprint(ui,response.rect,&self.recipe,shown.terrain.world_m);
                }
                if shown.view==View::Overview && response.clicked(){if let Some(pos)=response.interact_pointer_pos(){
                    let uv=(pos-response.rect.min)/response.rect.size();
                    self.recipe.inspection.x=(uv.x as f64*shown.terrain.world_m).clamp(0.,self.recipe.terrain.world_m-self.recipe.terrain.voxel_m());
                    self.recipe.inspection.z=(uv.y as f64*shown.terrain.world_m).clamp(0.,self.recipe.terrain.world_m-self.recipe.terrain.voxel_m());
                    self.status="Location selected. Choose Isometric, Local voxels or Cutaway and press Preview.".into();
                }}
            }
        } else {ui.label("Choose parameters and press Preview. Click a generated overview to select a local inspection location.");}
    });
    }
}
/// GUI overlay only: keep the sampled terrain texture reusable while panning.
fn draw_footprint(ui: &egui::Ui, map: egui::Rect, recipe: &Recipe, world: f64) {
    let i = &recipe.inspection;
    let s = recipe.terrain.voxel_m();
    let point = |x: f64, z: f64| {
        egui::pos2(
            map.left() + (x / world) as f32 * map.width(),
            map.top() + (z / world) as f32 * map.height(),
        )
    };
    let low = [
        ((i.x - i.span / 2.).max(0.) / s).ceil() * s,
        ((i.z - i.span / 2.).max(0.) / s).ceil() * s,
    ];
    let high = [
        ((i.x + i.span / 2.).min(world) / s).floor() * s,
        ((i.z + i.span / 2.).min(world) / s).floor() * s,
    ];
    let rect = egui::Rect::from_min_max(point(low[0], low[1]), point(high[0], high[1]));
    let painter = ui.painter().with_clip_rect(map);
    let red = egui::Stroke::new(2.0_f32, egui::Color32::RED);
    painter.rect_stroke(rect, 0., red, egui::StrokeKind::Outside);
    if rect.width() < 6. || rect.height() < 6. {
        let center = rect.center();
        painter.line_segment(
            [center - egui::vec2(7., 0.), center + egui::vec2(7., 0.)],
            red,
        );
        painter.line_segment(
            [center - egui::vec2(0., 7.), center + egui::vec2(0., 7.)],
            red,
        );
    }
}
fn save_recipe(r: &Recipe, path: &Path) -> Result<(), String> {
    r.validate()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
fn load_recipe(path: &Path) -> Result<Recipe, String> {
    let r: Recipe = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    r.validate()?;
    Ok(r)
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut recipe = Recipe::default();
    let mut headless = None;
    let mut export = None;
    let mut j = 0;
    while j < args.len() {
        let arg = &args[j];
        if arg == "--help" {
            println!("fbm [--params recipe.json] [--headless OUTPUT_DIRECTORY] [--view overview|local|isometric|slice] [--width 64..1024] [--export-voxels FILE]\nNo arguments: native GUI. Headless saves a PNG and complete recipe. --export-voxels writes a native 16 m fitting fixture.");
            return Ok(());
        }
        j += 1;
        let value = args
            .get(j)
            .ok_or_else(|| format!("Missing value for {arg}"))?;
        match arg.as_str() {
            "--params" => recipe = load_recipe(Path::new(value))?,
            "--headless" => headless = Some(PathBuf::from(value)),
            "--export-voxels" => export = Some(PathBuf::from(value)),
            "--width" => recipe.width = value.parse().map_err(|_| "Invalid width")?,
            "--view" => {
                recipe.view = match value.as_str() {
                    "overview" => View::Overview,
                    "local" => View::Local,
                    "isometric" => View::Isometric,
                    "slice" => View::Slice,
                    _ => return Err("Unknown view".into()),
                }
            }
            _ => return Err(format!("Unknown argument {arg}")),
        }
        j += 1;
    }
    recipe.validate()?;
    if let Some(path) = export {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let start = Instant::now();
        preview::export_voxels(&recipe, &path, &AtomicU64::new(1), 1)?;
        println!(
            "Saved {} ({:.2} s)",
            path.display(),
            start.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    if let Some(dir) = headless {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let start = Instant::now();
        let picture = preview::render(&recipe, Arc::new(AtomicU64::new(1)), 1)?;
        preview::save_png(&picture, &dir.join("preview.png"))?;
        save_recipe(&recipe, &dir.join("recipe.json"))?;
        println!(
            "{}\nSaved {} ({:.2} s)",
            picture.description,
            dir.display(),
            start.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200., 850.]),
        ..Default::default()
    };
    eframe::run_native(
        "fbm · Terrain authoring",
        options,
        Box::new(move |_| Ok(Box::new(App::new(recipe)))),
    )
    .map_err(|e| e.to_string())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("fbm: {e}");
        std::process::exit(1);
    }
}
