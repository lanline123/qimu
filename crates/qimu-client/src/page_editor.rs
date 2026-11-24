use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(super) const SHAPE_PRESETS: &[(&str, &str)] = &[
    ("rect", "矩形"),
    ("roundRect", "圆角矩形"),
    ("ellipse", "椭圆"),
    ("triangle", "三角形"),
    ("diamond", "菱形"),
    ("hexagon", "六边形"),
    ("rightArrow", "右箭头"),
];

pub(super) fn read_elements(elements: &[Value]) -> Vec<ElWork> {
    fn visit(values: &[Value], parents: &[usize], out: &mut Vec<ElWork>) {
        for (index, value) in values.iter().enumerate() {
            let mut path = parents.to_vec();
            path.push(index);
            let mut el = elwork_from_json(value);
            el.path = path.clone();
            out.push(el);
            if let Some(children) = value["children"].as_array() {
                visit(children, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(elements, &[], &mut out);
    out
}

#[derive(Clone, Copy)]
pub(super) enum AlignElements {
    Left,
    Horizontal,
    Right,
    Top,
    Vertical,
    Bottom,
}

pub(super) const ALIGNMENTS: &[(AlignElements, &str)] = &[
    (AlignElements::Left, "左对齐"),
    (AlignElements::Horizontal, "水平居中"),
    (AlignElements::Right, "右对齐"),
    (AlignElements::Top, "顶对齐"),
    (AlignElements::Vertical, "垂直居中"),
    (AlignElements::Bottom, "底对齐"),
];

impl PageEditState {
    pub(super) fn select_element(&mut self, index: Option<usize>, extend: bool) {
        if !extend || index.is_none() {
            self.selection.clear();
        }
        if let Some(index) = index {
            if extend && self.selection.contains(&index) {
                self.selection.remove(&index);
            } else {
                self.selection.insert(index);
            }
        }
        self.sel = index
            .filter(|index| self.selection.contains(index))
            .or_else(|| self.selection.iter().next_back().copied());
    }

    pub(super) fn added_pages(&self) -> usize {
        self.page_count - self.original_page_count
    }

    pub(super) fn add_page(&mut self) {
        self.push_undo();
        self.page = self.page_count;
        self.page_count += 1;
        self.work.insert(self.page, Vec::new());
        self.elems.insert(self.page, Vec::new());
        self.scripts.push(String::new());
        self.select_element(None, false);
    }

    pub(super) fn add_element(&mut self, kind: &str, picture: Option<(String, Vec<u8>)>) {
        self.push_undo();
        let page = self.elems.entry(self.page).or_default();
        let root_index = page.iter().filter(|el| el.path.len() == 1).count();
        let preset = match kind {
            "shape" | "text" => Some("rect".into()),
            "line" => Some("line".into()),
            _ => None,
        };
        let paras = if kind == "text" {
            vec![ParaWork {
                runs: vec![RunWork {
                    text: tr("新文本").into(),
                    ..RunWork::default()
                }],
            }]
        } else {
            Vec::new()
        };
        let position = 0.10 + (root_index % 5) as f32 * 0.04;
        page.push(ElWork {
            path: vec![root_index],
            kind: kind.into(),
            name: el_kind_label(kind).into(),
            x: (self.slide_cx as f32 * position) as i64,
            y: (self.slide_cy as f32 * position) as i64,
            cx: self.slide_cx / 4,
            cy: if matches!(kind, "line" | "text") {
                self.slide_cy / 12
            } else {
                self.slide_cy / 4
            },
            has_xfrm: true,
            preset,
            line_color: (kind == "line").then(|| "333333".into()),
            group_view: None,
            rot: 0,
            flip_h: false,
            flip_v: false,
            media: None,
            paras,
            new_image: picture,
        });
        let index = page.len() - 1;
        self.select_element(Some(index), false);
    }

    pub(super) fn can_align(&self) -> bool {
        let Some(elements) = self.elems.get(&self.page) else {
            return false;
        };
        let Some(first) = self
            .selection
            .iter()
            .next()
            .and_then(|index| elements.get(*index))
        else {
            return false;
        };
        self.selection.len() >= 2
            && self.selection.iter().all(|index| {
                elements
                    .get(*index)
                    .is_some_and(|el| el.has_xfrm && el.parents() == first.parents())
            })
    }

    pub(super) fn align(&mut self, direction: AlignElements) {
        if !self.can_align() {
            return;
        }
        self.push_undo();
        let elements = self.elems.get_mut(&self.page).unwrap();
        let mut min_x = i64::MAX;
        let mut min_y = i64::MAX;
        let mut max_x = i64::MIN;
        let mut max_y = i64::MIN;
        for &index in &self.selection {
            let el = &elements[index];
            min_x = min_x.min(el.x);
            min_y = min_y.min(el.y);
            max_x = max_x.max(el.x + el.cx);
            max_y = max_y.max(el.y + el.cy);
        }
        for &index in &self.selection {
            let el = &mut elements[index];
            match direction {
                AlignElements::Left => el.x = min_x,
                AlignElements::Horizontal => el.x = (min_x + max_x - el.cx) / 2,
                AlignElements::Right => el.x = max_x - el.cx,
                AlignElements::Top => el.y = min_y,
                AlignElements::Vertical => el.y = (min_y + max_y - el.cy) / 2,
                AlignElements::Bottom => el.y = max_y - el.cy,
            }
        }
    }
}

impl ElWork {
    pub(super) fn parents(&self) -> &[usize] {
        &self.path[..self.path.len().saturating_sub(1)]
    }

    /// The exact element insertion precedes all its subsequent edits on the wire.
    pub(super) fn seed_insert(&self, page: usize, out: &mut Vec<Value>) -> Self {
        let mut base = self.clone();
        let text = self
            .paras
            .first()
            .and_then(|para| para.runs.first())
            .map(|run| run.text.clone())
            .unwrap_or_default();
        let mut op = json!({
            "kind": "add_element", "page": page, "el": self.path.last(),
            "element_kind": self.kind, "preset": self.preset, "text": text,
            "x": self.x, "y": self.y, "cx": self.cx, "cy": self.cy,
        });
        if let Some((ext, bytes)) = &self.new_image {
            op["ext"] = json!(ext);
            op["data_base64"] = json!(b64e(bytes));
        }
        out.push(op);
        if self.kind == "text" {
            base.paras = vec![ParaWork {
                runs: vec![RunWork {
                    text,
                    ..RunWork::default()
                }],
            }];
        }
        if self.kind == "line" {
            base.line_color = Some("333333".into());
        }
        base
    }
}

pub(super) fn pick_editor_picture() -> Option<(String, Vec<u8>)> {
    let path = rfd::FileDialog::new()
        .add_filter("图片", &["png", "jpg", "jpeg", "gif", "bmp"])
        .pick_file()?;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some((ext, std::fs::read(path).ok()?))
}

#[derive(Clone, Copy)]
pub(super) struct StageTransform {
    pub origin: egui::Pos2,
    pub x_axis: egui::Vec2,
    pub y_axis: egui::Vec2,
}

impl StageTransform {
    fn vector(self, vector: egui::Vec2) -> egui::Vec2 {
        self.x_axis * vector.x + self.y_axis * vector.y
    }

    fn point(self, x: f32, y: f32) -> egui::Pos2 {
        self.origin + self.vector(egui::vec2(x, y))
    }

    pub(super) fn project(self, el: &ElWork) -> egui::Rect {
        let origin = egui::vec2(el.x as f32, el.y as f32);
        let size = egui::vec2(el.cx as f32, el.cy as f32);
        let rotation = egui::emath::Rot2::from_angle((el.rot as f32 / 60_000.0).to_radians());
        let mid = origin + size * 0.5;
        let points = [
            egui::Vec2::ZERO,
            egui::vec2(size.x, 0.0),
            size,
            egui::vec2(0.0, size.y),
        ]
        .map(|corner| {
            let p = mid + rotation * (corner - size * 0.5);
            self.point(p.x, p.y)
        });
        let mut area = egui::Rect::NOTHING;
        for point in points {
            area.extend_with(point);
        }
        if area.width() < 4.0 {
            area = area.expand2(egui::vec2(2.0, 0.0));
        }
        if area.height() < 4.0 {
            area = area.expand2(egui::vec2(0.0, 2.0));
        }
        area
    }

    pub(super) fn world_delta(self, point_delta: egui::Vec2) -> egui::Vec2 {
        let det = self.x_axis.x * self.y_axis.y - self.x_axis.y * self.y_axis.x;
        if det.abs() <= f32::EPSILON * f32::EPSILON {
            return egui::Vec2::ZERO;
        }
        egui::vec2(
            (point_delta.x * self.y_axis.y - point_delta.y * self.y_axis.x) / det,
            (self.x_axis.x * point_delta.y - self.x_axis.y * point_delta.x) / det,
        )
    }

    fn group(self, el: &ElWork) -> Self {
        let Some([x, y, cx, cy]) = el.group_view.filter(|rect| rect[2] > 0 && rect[3] > 0) else {
            return self;
        };
        let rotation = egui::emath::Rot2::from_angle((el.rot as f32 / 60_000.0).to_radians());
        let sx = el.cx as f32 / cx as f32 * if el.flip_h { -1.0 } else { 1.0 };
        let sy = el.cy as f32 / cy as f32 * if el.flip_v { -1.0 } else { 1.0 };
        let x_axis = self.vector(rotation * egui::vec2(sx, 0.0));
        let y_axis = self.vector(rotation * egui::vec2(0.0, sy));
        let center = self.point(
            el.x as f32 + el.cx as f32 / 2.0,
            el.y as f32 + el.cy as f32 / 2.0,
        );
        Self {
            origin: center
                - x_axis * (x as f32 + cx as f32 * 0.5)
                - y_axis * (y as f32 + cy as f32 * 0.5),
            x_axis,
            y_axis,
        }
    }
}

pub(super) fn stage_transforms(elements: &[ElWork], root: StageTransform) -> Vec<StageTransform> {
    let mut groups = BTreeMap::new();
    elements
        .iter()
        .map(|el| {
            let parent = groups.get(el.parents()).copied().unwrap_or(root);
            if el.kind == "group" {
                groups.insert(el.path.as_slice(), parent.group(el));
            }
            parent
        })
        .collect()
}

type PreviewResult = Result<egui::ColorImage, String>;

#[derive(Default)]
pub(super) struct EditStagePreview {
    desired: (usize, u64),
    revision: u64,
    ready_at: Option<Instant>,
    request: Option<PreviewRequest>,
    rendered: Option<(usize, egui::TextureHandle)>,
    error: Option<String>,
    extra_pages: usize,
    page_elements: Vec<ElWork>,
}

struct PreviewRequest {
    key: (usize, u64),
    receiver: mpsc::Receiver<PreviewResult>,
}

impl App {
    pub(super) fn update_page_edit_preview(&self, ctx: &egui::Context, pe: &mut PageEditState) {
        let now = Instant::now();
        let page = pe.page;
        let elements = pe.elems.get(&page).map(Vec::as_slice).unwrap_or(&[]);
        let base = pe.elems_base.get(&page).map(Vec::as_slice).unwrap_or(&[]);
        let dirty =
            pe.elems.contains_key(&page) && (page >= pe.original_page_count || elements != base);
        let added = pe.added_pages();
        let preview = &mut pe.preview;
        if preview.desired.0 != page
            || preview.page_elements != elements
            || preview.extra_pages != added
        {
            preview.revision = preview.revision.wrapping_add(1);
            preview.desired = (page, preview.revision);
            preview.page_elements = elements.to_vec();
            preview.extra_pages = added;
            preview.ready_at = dirty.then_some(now + Duration::from_millis(240));
            preview.error = None;
        }
        if !dirty {
            preview.rendered = None;
            preview.ready_at = None;
        }
        if let Some(request) = &preview.request {
            if let Ok(result) = request.receiver.try_recv() {
                if request.key == preview.desired && dirty {
                    match result {
                        Ok(image) => {
                            preview.rendered = Some((
                                page,
                                ctx.load_texture(
                                    "page-editor-stage",
                                    image,
                                    egui::TextureOptions::LINEAR,
                                ),
                            ));
                            preview.error = None;
                        }
                        Err(error) => preview.error = Some(error),
                    }
                }
                preview.request = None;
            }
        }
        if preview.request.is_some() {
            return;
        }
        let Some(ready_at) = preview.ready_at else {
            return;
        };
        if now < ready_at {
            ctx.request_repaint_after(ready_at - now);
            return;
        }
        let key = preview.desired;
        preview.ready_at = None;
        let request =
            json!({ "preview_page": page, "added_pages": added, "ops": pe.page_ops(Some(page)) });
        let server = self.server.clone();
        let token = self.token.clone();
        let path = format!("/v1/versions/{}/edit-pages", pe.version_id);
        let (tx, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = raw_call(
                &server,
                token.as_deref(),
                "POST",
                &path,
                Some(request),
                false,
            )
            .and_then(|response| b64d(&s(&response, "data_base64")))
            .and_then(|bytes| {
                let image = image::load_from_memory(&bytes)
                    .map_err(|error| error.to_string())?
                    .into_rgba8();
                Ok(egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ))
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        pe.preview.request = Some(PreviewRequest { key, receiver });
    }
}

impl EditStagePreview {
    pub(super) fn texture(&self, page: usize) -> Option<egui::TextureHandle> {
        self.rendered
            .as_ref()
            .filter(|(index, _)| *index == page)
            .map(|(_, texture)| texture.clone())
    }

    pub(super) fn status(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.label(
                egui::RichText::new(format!("{} · {error}", tr("舞台预览失败")))
                    .color(palette_color(BADGE_BAD_FG)),
            );
        } else if self.ready_at.is_some() || self.request.is_some() {
            ui.label(egui::RichText::new(tr("正在更新舞台…")).weak());
        }
    }
}
