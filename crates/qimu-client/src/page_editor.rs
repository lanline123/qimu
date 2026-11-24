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
        // Transplant T0: with the local library open, the stage
        // preview is produced in-process (preview_png_in_process:
        // the same core ops application the server performs, then
        // local_render at scale 1 / edit mode). Any case it does not
        // cover — no local library, version not local, a local
        // pipeline failure — falls back to the server request below.
        let local = self.local.clone();
        let version_id = pe.version_id.clone();
        std::thread::spawn(move || {
            let result = local
                .as_deref()
                .and_then(|store| {
                    preview_png_in_process(store, &version_id, page, added, &request["ops"])
                })
                .map_or_else(
                    || {
                        raw_call(
                            &server,
                            token.as_deref(),
                            "POST",
                            &path,
                            Some(request),
                            false,
                        )
                        .and_then(|response| b64d(&s(&response, "data_base64")))
                    },
                    Ok,
                )
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

/// Sniff an image format from magic bytes (None = unrecognized).
/// Mirrors the server's helper of the same name (edit-pages).
fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Convert the wire ops of one edit-pages request (the JSON shape
/// `PageEditState::page_ops` emits) into core `SlideOp`s grouped by
/// page — a line-for-line mirror of the conversion the server
/// performs in its edit-pages handler, so the in-process preview
/// applies exactly the operations the server would.
pub(super) fn slide_ops_from_json(
    ops: &[Value],
) -> Result<std::collections::BTreeMap<usize, Vec<qimu_core::pptx::SlideOp>>, String> {
    use qimu_core::pptx::SlideOp;
    if ops.len() > 20_000 {
        return Err("编辑操作过多".into());
    }
    let str_of = |op: &Value, key: &str| -> Option<String> {
        op.get(key).and_then(|v| v.as_str()).map(str::to_string)
    };
    let usize_of = |op: &Value, key: &str| -> Option<usize> {
        op.get(key).and_then(|v| v.as_u64()).map(|n| n as usize)
    };
    let i64_of = |op: &Value, key: &str| -> Option<i64> { op.get(key).and_then(|v| v.as_i64()) };
    // Picture payloads are decoded and sniffed up front, exactly
    // like the server: op index -> (bytes, ext).
    let mut pic_bytes: std::collections::HashMap<usize, (Vec<u8>, String)> =
        std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        let kind = str_of(op, "kind").unwrap_or_default();
        let needs_picture = kind == "replace_pic"
            || (kind == "add_element" && str_of(op, "element_kind").as_deref() == Some("picture"));
        if !needs_picture {
            continue;
        }
        let b64 = str_of(op, "data_base64").ok_or("替换图片缺少图片数据")?;
        let bytes = b64d(&b64)?;
        let sniffed = sniff_image_ext(&bytes).ok_or("只支持 PNG / JPEG / GIF / BMP 图片")?;
        let ext = match str_of(op, "ext").map(|e| e.to_ascii_lowercase()) {
            Some(e) if e == sniffed || (e == "jpeg" && sniffed == "jpg") => {
                sniffed.to_string()
            }
            Some(_) => return Err("图片扩展名与实际格式不一致".into()),
            None => sniffed.to_string(),
        };
        pic_bytes.insert(i, (bytes, ext));
    }
    let mut ops_by_page: std::collections::BTreeMap<usize, Vec<SlideOp>> =
        std::collections::BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        let kind = str_of(op, "kind").unwrap_or_default();
        let el = usize_of(op, "el").unwrap_or(0);
        let para = usize_of(op, "para");
        let sop = match kind.as_str() {
            "set_text" => SlideOp::SetText {
                el,
                para: para.unwrap_or(0),
                run: usize_of(op, "run").unwrap_or(0),
                text: str_of(op, "text").unwrap_or_default(),
            },
            "set_style" => SlideOp::SetStyle {
                el,
                para: para.unwrap_or(0),
                run: usize_of(op, "run").unwrap_or(0),
                sz: op.get("sz").and_then(|v| v.as_f64()).map(|f| f as f32),
                b: op.get("b").and_then(|v| v.as_bool()),
                color: str_of(op, "color"),
            },
            "add_para" => SlideOp::AddPara {
                el,
                after: para,
                text: str_of(op, "text").unwrap_or_default(),
            },
            "del_para" => SlideOp::DeletePara {
                el,
                para: para.unwrap_or(0),
            },
            "add_run" => SlideOp::AddRun {
                el,
                para: para.unwrap_or(0),
                text: str_of(op, "text").unwrap_or_default(),
            },
            "del_run" => SlideOp::DeleteRun {
                el,
                para: para.unwrap_or(0),
                run: usize_of(op, "run").unwrap_or(0),
            },
            "move" => SlideOp::Move {
                el,
                x: i64_of(op, "x").unwrap_or(0),
                y: i64_of(op, "y").unwrap_or(0),
            },
            "resize" => SlideOp::Resize {
                el,
                cx: i64_of(op, "cx").unwrap_or(0),
                cy: i64_of(op, "cy").unwrap_or(0),
            },
            "reset_style" => SlideOp::ResetStyle {
                el,
                para: para.unwrap_or(0),
                run: usize_of(op, "run").unwrap_or(0),
            },
            "line_color" => SlideOp::LineColor {
                el,
                color: str_of(op, "color").ok_or("缺少线条颜色")?,
            },
            "preset" => {
                let preset = str_of(op, "preset")
                    .filter(|p| qimu_core::pptx::editor_preset_allowed(p))
                    .ok_or("未知的形状预设")?;
                SlideOp::Preset { el, preset }
            }
            "add_element" => SlideOp::AddElement(qimu_core::pptx::NewSlideElement {
                kind: str_of(op, "element_kind").ok_or("缺少元素类型")?,
                preset: str_of(op, "preset"),
                text: str_of(op, "text").unwrap_or_default(),
                x: i64_of(op, "x").unwrap_or(0),
                y: i64_of(op, "y").unwrap_or(0),
                cx: i64_of(op, "cx").unwrap_or(0),
                cy: i64_of(op, "cy").unwrap_or(0),
                picture: pic_bytes.remove(&i),
            }),
            "replace_pic" => {
                let (bytes, ext) = pic_bytes
                    .get(&i)
                    .ok_or("替换图片数据丢失")?
                    .clone();
                SlideOp::ReplacePic { el, bytes, ext }
            }
            other => return Err(format!("未知编辑操作 {other}")),
        };
        let parents: Vec<usize> = op
            .get("parents")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as usize))
                    .collect()
            })
            .unwrap_or_default();
        if parents.len() > 32 {
            return Err("组合层级过深".into());
        }
        let sop = if parents.is_empty() {
            sop
        } else {
            SlideOp::InGroup {
                parents,
                op: Box::new(sop),
            }
        };
        ops_by_page
            .entry(usize_of(op, "page").unwrap_or(0))
            .or_default()
            .push(sop);
    }
    Ok(ops_by_page)
}

/// In-process stage preview (transplant T0): replay the server
/// edit-pages preview pipeline against the local library — load the
/// version's fragment, append the added pages, apply the request's
/// v2 ops with qimu-core, then render the page via local_render.
/// Returns None when the version is not in the local library or any
/// step fails; the caller then falls back to the server request.
fn preview_png_in_process(
    local: &crate::local_store::LocalStore,
    version_id: &str,
    page: usize,
    added_pages: usize,
    ops: &Value,
) -> Option<Vec<u8>> {
    let frag = local.version_fragment(version_id)?;
    let mut pkg = qimu_core::pptx::Package::load(&frag).ok()?;
    qimu_core::pptx::append_blank_slides(&mut pkg, added_pages).ok()?;
    let ops_by_page = slide_ops_from_json(ops.as_array()?).ok()?;
    for (p, list) in &ops_by_page {
        qimu_core::pptx::apply_slide_ops(&mut pkg, *p, list).ok()?;
    }
    if page >= qimu_core::pptx::slides(&pkg).ok()?.len() {
        return None;
    }
    let bytes = pkg.to_bytes().ok()?;
    crate::local_render::render_edit_preview(&bytes, page).ok()
}
