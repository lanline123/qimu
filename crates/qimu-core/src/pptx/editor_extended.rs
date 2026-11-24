use super::*;

const PPT_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// Append blank slides sharing the source layout. Existing slide parts and relationship IDs stay intact.
pub fn append_blank_slides(pkg: &mut Package, count: usize) -> PptxResult<()> {
    if count == 0 {
        return Ok(());
    }
    if count > 100 {
        return Err("一次最多添加 100 页".into());
    }
    let source = slides(pkg)?.pop().ok_or("找不到可复用的幻灯片版式")?;
    let layout = parse_rels(&pkg.get_text(&rels_part_name(&source.part))?)
        .into_iter()
        .find(|rel| !rel.external && rel.ty.ends_with("/slideLayout"))
        .ok_or("找不到可复用的幻灯片版式")?;
    let layout_part = resolve_target(&source.part, &layout.target);
    let sld_rels = rels_xml(&[("rId1".into(), layout.ty, format!("/{layout_part}"), false)]);
    let mut pres = parse_xml(&pkg.get_text(PRESENTATION)?, PRESENTATION)?;
    let list = pres.child_mut(Local("sldIdLst")).ok_or("缺少幻灯片列表")?;
    let mut id = list
        .children(Local("sldId"))
        .filter_map(|node| node.attr("id").and_then(|value| value.parse::<u32>().ok()))
        .max()
        .unwrap_or(255);
    let mut rels: Vec<_> = parse_rels(&pkg.get_text(PRESENTATION_RELS)?)
        .into_iter()
        .map(|rel| (rel.id, rel.ty, rel.target, rel.external))
        .collect();
    let mut types = parse_xml(&pkg.get_text(CONTENT_TYPES)?, CONTENT_TYPES)?;
    let mut number = 1u32;
    for _ in 0..count {
        let (part, rid) = loop {
            let part = format!("ppt/slides/qimu-added-{number}.xml");
            let rid = format!("rIdQimuSlide{number}");
            number = number.checked_add(1).ok_or("幻灯片编号已耗尽")?;
            if !pkg.parts.contains_key(&part) && !rels.iter().any(|rel| rel.0 == rid) {
                break (part, rid);
            }
        };
        id = id
            .checked_add(1)
            .filter(|id| *id < 0x8000_0000)
            .ok_or("幻灯片编号已耗尽")?;
        list.children.push(Node::El(
            El::new("p:sldId")
                .with_attr("id", id)
                .with_attr("r:id", &rid),
        ));
        rels.push((rid, format!("{PPT_REL}/slide"), format!("/{part}"), false));
        types.children.push(Node::El(
            El::new("Override")
                .with_attr("PartName", format!("/{part}"))
                .with_attr(
                    "ContentType",
                    "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
                ),
        ));
        let xml = format!(
            r#"<p:sld xmlns:p="{P_NS}" xmlns:a="{A_NS}" xmlns:r="{PPT_REL}" showMasterSp="0"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        );
        pkg.parts
            .insert(rels_part_name(&part), sld_rels.as_bytes().to_vec());
        pkg.parts.insert(part, xml.into_bytes());
    }
    pkg.parts.insert(PRESENTATION.into(), serialize_xml(&pres));
    pkg.parts
        .insert(PRESENTATION_RELS.into(), rels_xml(&rels).into_bytes());
    pkg.parts
        .insert(CONTENT_TYPES.into(), serialize_xml(&types));
    Ok(())
}

pub fn editor_preset_allowed(preset: &str) -> bool {
    matches!(
        preset,
        "rect" | "roundRect" | "ellipse" | "triangle" | "diamond" | "hexagon" | "rightArrow"
    )
}

pub(super) fn group_inner_span(xml: &str) -> Option<(usize, usize)> {
    let (_, _, open_end, _, _) = next_tag(xml, 0)?;
    Some((open_end, xml.rfind("</p:grpSp>")?))
}

pub(super) fn span_at(xml: &str, parents: &[usize], el: usize) -> Option<(usize, usize)> {
    let (mut lo, mut hi) = sp_tree_inner(xml)?;
    for &parent in parents {
        let (tag, start, end) = child_spans(&xml[lo..hi], &EL_TAGS)
            .into_iter()
            .nth(parent)?;
        if tag != "p:grpSp" {
            return None;
        }
        let base = lo + start;
        let (inside, close) = group_inner_span(&xml[base..lo + end])?;
        lo = base + inside;
        hi = base + close;
    }
    child_spans(&xml[lo..hi], &EL_TAGS)
        .into_iter()
        .nth(el)
        .map(|(_, start, end)| (lo + start, lo + end))
}

const FILL_TAGS: [&str; 6] = [
    "a:noFill",
    "a:solidFill",
    "a:gradFill",
    "a:blipFill",
    "a:pattFill",
    "a:grpFill",
];

pub(super) fn line_color(block: &str) -> Option<String> {
    let start = block.find("<a:ln")?;
    let (_, _, end, _, self_closing) = next_tag(block, start)?;
    if self_closing {
        return None;
    }
    let stop = end + block[end..].find("</a:ln>")?;
    find_tag_attr(&block[end..stop], "<a:srgbClr", "val")
}

pub(super) fn replace_element_fill(
    block: &str,
    start: usize,
    name: &str,
    fill: &str,
) -> Option<String> {
    let (_, _, end, _, self_closing) = next_tag(block, start)?;
    if self_closing {
        if fill.is_empty() {
            return Some(block.to_owned());
        }
        let replacement = format!("{}>{fill}</{name}>", &block[start..end - 2]);
        return Some(splice(block, (start, end), &replacement));
    }
    let close = end + block[end..].find(&format!("</{name}>"))?;
    let mut contents = block[end..close].to_owned();
    for (_, a, b) in child_spans(&contents, &FILL_TAGS).into_iter().rev() {
        contents.replace_range(a..b, "");
    }
    contents.insert_str(0, fill);
    Some(splice(block, (end, close), &contents))
}

pub(super) fn valid_rgb(color: &str) -> bool {
    color.len() == 6 && color.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn set_line_color(block: &str, color: &str) -> Option<String> {
    let fill = format!("<a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill>");
    if let Some(start) = find_tag_text(block, "<a:ln").and_then(|_| block.find("<a:ln")) {
        return replace_element_fill(block, start, "a:ln", &fill);
    }
    let end = block.find("</p:spPr>")?;
    Some(splice(block, (end, end), &format!("<a:ln>{fill}</a:ln>")))
}

pub(super) fn reset_run_style(block: &str, para: usize, run: usize) -> Option<String> {
    let paragraph = *para_spans(block).get(para)?;
    let span = *run_spans(block, paragraph).get(run)?;
    let rb = &block[span.0..span.1];
    let tag = find_tag_text(rb, "<a:rPr")?;
    let start = rb.find("<a:rPr")?;
    let input = format!("{tag}{}", if tag.ends_with("/>") { "" } else { "</a:rPr>" });
    let mut props = parse_xml(&input, "a:rPr").ok()?;
    props.remove_attr("sz");
    props.remove_attr("b");
    // Keep any local namespace declarations and unrelated formatting attributes.
    let new_tag = serialize_xml(&props);
    let new_tag = String::from_utf8(new_tag).ok()?;
    let new_tag = find_tag_text(&new_tag, "<a:rPr")?;
    let normalized;
    let new_tag = if !tag.ends_with("/>") && new_tag.ends_with("/>") {
        normalized = format!("{}>", &new_tag[..new_tag.len() - 2]);
        &normalized
    } else {
        new_tag
    };
    let changed = splice(rb, (start, start + tag.len()), new_tag);
    let changed = replace_element_fill(&changed, start, "a:rPr", "")?;
    Some(splice(block, span, &changed))
}

pub(super) fn change_preset(block: &str, preset: &str) -> Option<String> {
    if !editor_preset_allowed(preset) {
        return None;
    }
    // Replace the preset as a unit so adjustment handles from the old shape do not carry over.
    let start = block.find("<a:prstGeom")?;
    let (_, _, open_end, _, sc) = next_tag(block, start)?;
    let end = if sc {
        open_end
    } else {
        open_end + block[open_end..].find("</a:prstGeom>")? + "</a:prstGeom>".len()
    };
    Some(splice(
        block,
        (start, end),
        &format!("<a:prstGeom prst=\"{preset}\"><a:avLst/></a:prstGeom>"),
    ))
}

pub struct NewSlideElement {
    pub kind: String,
    pub preset: Option<String>,
    pub text: String,
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    pub picture: Option<(Vec<u8>, String)>,
}

pub(super) fn add_element(
    pkg: &mut Package,
    part: &str,
    xml: &str,
    el: &NewSlideElement,
) -> PptxResult<String> {
    let (_, end) = sp_tree_inner(xml).ok_or("页面缺少形状树")?;
    let mut start = 0;
    let mut max_id = 1u32;
    while let Some((tag, a, b, closing, _)) = next_tag(xml, start) {
        start = b;
        if tag == "p:cNvPr" && !closing {
            if let Some(id) = attr(&xml[a..b], "id").and_then(|id| id.parse::<u32>().ok()) {
                max_id = max_id.max(id);
            }
        }
    }
    let id = max_id.checked_add(1).ok_or("页面元素编号已耗尽")?;
    let NewSlideElement { x, y, cx, cy, .. } = el;
    if *cx <= 0 || *cy <= 0 {
        return Err("元素宽高必须大于零".into());
    }
    let xfrm =
        format!("<a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>");
    let mut text = String::new();
    if el.kind == "text" {
        text = format!(
            "<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody>",
            xml_escape_text(&el.text)
        );
    }
    let insert = match el.kind.as_str() {
        "text" | "shape" | "line" => {
            let (preset, fill, textbox) = match el.kind.as_str() {
                "text" => ("rect", "<a:noFill/><a:ln><a:noFill/></a:ln>", " txBox=\"1\""),
                "line" => ("line", "<a:ln w=\"25400\"><a:solidFill><a:srgbClr val=\"333333\"/></a:solidFill></a:ln>", ""),
                _ => {
                    let preset = el.preset.as_deref().unwrap_or("rect");
                    if !editor_preset_allowed(preset) { return Err("未知的形状预设".into()); }
                    (preset, "<a:solidFill><a:srgbClr val=\"2F6BF5\"/></a:solidFill>", "")
                }
            };
            format!("<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Element {id}\"/><p:cNvSpPr{textbox}/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm}<a:prstGeom prst=\"{preset}\"><a:avLst/></a:prstGeom>{fill}</p:spPr>{text}</p:sp>")
        }
        "picture" => {
            let (bytes, ext) = el.picture.as_ref().ok_or("新增图片缺少图片数据")?;
            let rid = add_picture_relationship(pkg, part, bytes, ext)?;
            format!("<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {id}\"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{xfrm}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>")
        }
        _ => return Err("未知的元素类型".into()),
    };
    // The extension list, when present, is the final child of a shape tree.
    let (first, _) = sp_tree_inner(xml).unwrap();
    let at = child_spans(&xml[first..end], &["p:extLst"])
        .first()
        .map_or(end, |(_, pos, _)| first + pos);
    Ok(splice(xml, (at, at), &insert))
}

pub(super) fn add_picture_relationship(
    pkg: &mut Package,
    part: &str,
    bytes: &[u8],
    ext: &str,
) -> PptxResult<String> {
    if !matches!(ext, "png" | "jpg" | "gif" | "bmp") {
        return Err("未支持的图片格式".into());
    }
    let media = format!("ppt/media/qimu_{:x}.{ext}", Sha256::digest(bytes));
    let rel_part = rels_part_name(part);
    let mut rels: Vec<_> = parse_rels(&pkg.get_text(&rel_part).unwrap_or_default())
        .into_iter()
        .map(|rel| (rel.id, rel.ty, rel.target, rel.external))
        .collect();
    let rid = (1..)
        .map(|number| format!("rIdQimuPicture{number}"))
        .find(|id| !rels.iter().any(|rel| &rel.0 == id))
        .ok_or("图片关系编号已耗尽")?;
    rels.push((
        rid.clone(),
        format!("{PPT_REL}/image"),
        format!("/{media}"),
        false,
    ));
    pkg.parts.insert(rel_part, rels_xml(&rels).into_bytes());
    pkg.parts.entry(media).or_insert_with(|| bytes.to_vec());
    ensure_content_type(pkg, ext);
    Ok(rid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_removes_editable_overrides_but_keeps_other_properties() {
        let shape = r#"<p:sp><p:txBody><a:p><a:r><a:rPr sz="1800" b="1" i="1"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:latin typeface="Example"/></a:rPr><a:t>text</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let reset = reset_run_style(shape, 0, 0).unwrap();
        assert!(!reset.contains("sz="));
        assert!(!reset.contains(" b="));
        assert!(!reset.contains("solidFill"));
        assert!(reset.contains(r#"i="1""#));
        assert!(reset.contains(r#"<a:latin typeface="Example"/>"#));
        assert!(reset.contains("<a:t>text</a:t>"));
    }

    #[test]
    fn nested_path_targets_only_the_requested_child() {
        let xml = r#"<p:sld><p:spTree><p:grpSp><p:sp/><p:grpSp><p:cxnSp name="target"/></p:grpSp></p:grpSp><p:sp name="root"/></p:spTree></p:sld>"#;
        let (start, end) = span_at(xml, &[0, 1], 0).unwrap();
        assert_eq!(&xml[start..end], r#"<p:cxnSp name="target"/>"#);
        assert!(span_at(xml, &[1], 0).is_none());
    }
}
