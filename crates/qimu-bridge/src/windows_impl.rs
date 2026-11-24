//! Windows COM automation implementation (spec §4.3).
//!
//! Drives the running PowerPoint through late-bound IDispatch calls on
//! `PowerPoint.Application`. Only the running instance is used
//! (`GetActiveObject`); the bridge never starts a hidden PowerPoint of its
//! own for editing, because the whole point is to modify the presentation
//! the user already has open.
//!
//! Replacement sequence per §5.3, applied in descending position order so
//! earlier indices stay valid:
//!   1. delete `remove_count` slides at `start` (Slides.Item(start).Delete)
//!   2. `Slides.InsertFromFile(fragment, start - 1, 1, -1)` inserts the
//!      fragment's slides so the first one lands at position `start`
//! The presentation is NOT saved by the bridge — the file on disk changes
//! only when the user saves in PowerPoint, exactly like a manual edit.

use windows::core::{BSTR, GUID, Interface, PCWSTR, VARIANT};
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPPARAMS,
};
use windows::Win32::System::Ole::GetActiveObject;

use crate::{BridgeStatus, PowerPointBridge, ReplaceReport, SlideReplacement};

pub struct WindowsBridge;

struct ComGuard;
impl ComGuard {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| format!("COM 初始化失败: {e}"))?;
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

fn dispid(obj: &IDispatch, name: &str) -> Result<i32, String> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let names = [PCWSTR(wide.as_ptr())];
    let mut id = 0i32;
    unsafe {
        obj.GetIDsOfNames(
            &GUID::zeroed(),
            names.as_ptr(),
            1,
            0x0400, // LOCALE_USER_DEFAULT-ish; PowerPoint accepts 0 (LOCALE_SYSTEM_DEFAULT works too)
            &mut id,
        )
    }
    .map_err(|e| format!("GetIDsOfNames({name}) 失败: {e}"))?;
    Ok(id)
}

fn invoke(
    obj: &IDispatch,
    name: &str,
    flags: windows::Win32::System::Com::DISPATCH_FLAGS,
    args: &mut [VARIANT],
) -> Result<VARIANT, String> {
    let id = dispid(obj, name)?;
    // IDispatch method arguments are passed in reverse order.
    args.reverse();
    let params = DISPPARAMS {
        rgvarg: if args.is_empty() {
            std::ptr::null_mut()
        } else {
            args.as_mut_ptr()
        },
        rgdispidNamedArgs: std::ptr::null_mut(),
        cArgs: args.len() as u32,
        cNamedArgs: 0,
    };
    let mut result = VARIANT::default();
    unsafe {
        obj.Invoke(
            id,
            &GUID::zeroed(),
            0x0400,
            flags,
            &params,
            Some(&mut result),
            None,
            None,
        )
    }
    .map_err(|e| format!("Invoke({name}) 失败: {e}"))?;
    Ok(result)
}

fn get_prop(obj: &IDispatch, name: &str) -> Result<VARIANT, String> {
    invoke(obj, name, DISPATCH_PROPERTYGET, &mut [])
}

fn get_obj(obj: &IDispatch, name: &str) -> Result<IDispatch, String> {
    let v = get_prop(obj, name)?;
    IDispatch::try_from(&v).map_err(|e| format!("属性 {name} 不是对象: {e}"))
}

fn get_string(obj: &IDispatch, name: &str) -> Result<String, String> {
    let v = get_prop(obj, name)?;
    let b = BSTR::try_from(&v).map_err(|e| format!("属性 {name} 不是字符串: {e}"))?;
    Ok(b.to_string())
}

fn get_i32(obj: &IDispatch, name: &str) -> Result<i32, String> {
    let v = get_prop(obj, name)?;
    i32::try_from(&v).map_err(|e| format!("属性 {name} 不是整数: {e}"))
}

fn call(obj: &IDispatch, name: &str, args: &mut [VARIANT]) -> Result<VARIANT, String> {
    invoke(obj, name, DISPATCH_METHOD, args)
}

/// The running PowerPoint.Application, or an error if none is running.
fn active_powerpoint() -> Result<IDispatch, String> {
    let progid = BSTR::from("PowerPoint.Application");
    let clsid = unsafe { CLSIDFromProgID(PCWSTR(progid.as_ptr())) }
        .map_err(|e| format!("PowerPoint 未安装或 ProgID 解析失败: {e}"))?;
    let mut unknown: Option<windows::core::IUnknown> = None;
    if unsafe { GetActiveObject(&clsid, None, &mut unknown) }.is_ok() {
        if let Some(unk) = unknown {
            return unk
                .cast::<IDispatch>()
                .map_err(|e| format!("获取 PowerPoint 自动化接口失败: {e}"));
        }
    }
    // Fallback: CoCreateInstance binds to the running instance for
    // single-instance applications like PowerPoint.
    unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) }
        .map_err(|e| format!("PowerPoint 未在运行: {e}"))
}

fn active_presentation(app: &IDispatch) -> Result<IDispatch, String> {
    get_obj(app, "ActivePresentation")
        .map_err(|_| "PowerPoint 中没有打开的演示文稿".to_string())
}

impl PowerPointBridge for WindowsBridge {
    fn status(&self) -> BridgeStatus {
        let _com = ComGuard::new();
        match active_powerpoint() {
            Ok(app) => {
                let version = get_string(&app, "Version").ok();
                let active_path = active_presentation(&app)
                    .and_then(|p| get_string(&p, "FullName"))
                    .ok();
                BridgeStatus {
                    host: "powerpoint".into(),
                    running: true,
                    version,
                    active_path,
                    detail: "已连接到正在运行的 PowerPoint".into(),
                }
            }
            Err(e) => BridgeStatus {
                host: "powerpoint".into(),
                running: false,
                version: None,
                active_path: None,
                detail: e,
            },
        }
    }

    fn replace_slides_in_place(
        &self,
        expected_path: &str,
        replacements: &[SlideReplacement],
    ) -> Result<ReplaceReport, String> {
        let _com = ComGuard::new()?;
        let app = active_powerpoint()?;
        let pres = active_presentation(&app)?;
        let open_path = get_string(&pres, "FullName")?;
        if !open_path.eq_ignore_ascii_case(expected_path) {
            return Err(format!(
                "当前打开的文稿是「{open_path}」，与要更新的「{expected_path}」不是同一份，已停止"
            ));
        }
        let slides = get_obj(&pres, "Slides")?;
        let before = get_i32(&slides, "Count")? as usize;

        // Descending order: replacements below never shift the positions of
        // the ones still to be applied above them.
        let mut ordered: Vec<&SlideReplacement> = replacements.iter().collect();
        ordered.sort_by_key(|r| std::cmp::Reverse(r.start));
        for rep in &ordered {
            for _ in 0..rep.remove_count {
                let item = call(
                    &slides,
                    "Item",
                    &mut [VARIANT::from(rep.start as i32)],
                )?;
                let slide = IDispatch::try_from(&item)
                    .map_err(|e| format!("Slides.Item 不是对象: {e}"))?;
                call(&slide, "Delete", &mut [])?;
            }
            if let Some(fragment) = &rep.fragment_path {
                // InsertFromFile(FileName, Index, SlideStart, SlideEnd):
                // Index = number of slides before the insertion point, so
                // start-1 puts the first fragment slide at position `start`.
                // SlideEnd -1 = through the fragment's last slide.
                call(
                    &slides,
                    "InsertFromFile",
                    &mut [
                        VARIANT::from(BSTR::from(fragment.as_str())),
                        VARIANT::from((rep.start - 1) as i32),
                        VARIANT::from(1i32),
                        VARIANT::from(-1i32),
                    ],
                )?;
            }
        }
        let after = get_i32(&slides, "Count")? as usize;
        Ok(ReplaceReport {
            path: open_path,
            applied: replacements.len(),
            slides_before: before,
            slides_after: after,
        })
    }

    /// Clipboard capture route (chosen for robustness over driving the
    /// user's own selection, which would mutate their open deck):
    ///   1. `Presentations.Add(msoFalse)` creates a hidden throwaway
    ///      presentation — the user's open files are never touched.
    ///   2. `Slides.Paste()` pastes whatever slides are on the clipboard.
    ///      It raises when the clipboard holds no slides (e.g. plain
    ///      text), which maps to `Ok(None)` — "nothing copied".
    ///   3. `Presentation.SaveAs(path, ppSaveAsOpenXMLPresentation = 24)`
    ///      writes the fragment; `Close()` discards the throwaway deck.
    /// The copied slides keep their original design because Paste into a
    /// new presentation carries the source theme with the slides.
    /// NOTE: written against the documented object model; like the rest
    /// of this bridge it has NOT yet run on a real Windows + PowerPoint
    /// machine (first live check pending).
    fn capture_copied_slides(&self) -> Result<Option<std::path::PathBuf>, String> {
        let _com = ComGuard::new()?;
        let app = match active_powerpoint() {
            Ok(a) => a,
            Err(_) => return Ok(None),
        };
        let presentations = get_obj(&app, "Presentations")?;
        // Add(WithWindow = msoFalse = 0): no window for the temp deck.
        let added = call(&presentations, "Add", &mut [VARIANT::from(0i32)])?;
        let pres = IDispatch::try_from(&added)
            .map_err(|e| format!("新建临时演示失败: {e}"))?;
        let slides = get_obj(&pres, "Slides")?;
        if call(&slides, "Paste", &mut []).is_err()
            || get_i32(&slides, "Count").unwrap_or(0) == 0
        {
            let _ = call(&pres, "Close", &mut []);
            return Ok(None);
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("qimu-paste-{stamp}.pptx"));
        let saved = call(
            &pres,
            "SaveAs",
            &mut [
                VARIANT::from(BSTR::from(path.to_string_lossy().as_ref())),
                VARIANT::from(24i32), // ppSaveAsOpenXMLPresentation
            ],
        );
        let _ = call(&pres, "Close", &mut []);
        saved?;
        Ok(Some(path))
    }

    /// Clipboard publish route — the mirror of capture. PowerPoint's
    /// slide clipboard format is its own private serialization, which
    /// no third party can construct reliably, so PowerPoint itself
    /// renders it: the fragment is opened in a hidden throwaway
    /// presentation, all of its slides are copied by PowerPoint, and
    /// the throwaway is closed without saving. The user's open files
    /// are never touched. Only a running instance is used (the rule
    /// for this whole bridge): with no PowerPoint running this
    /// returns `Ok(false)` and the caller keeps the fragment in the
    /// in-app clipboard only.
    /// NOTE: written against the documented object model; like the
    /// rest of this bridge it has NOT yet run on a real Windows +
    /// PowerPoint machine (first live check pending). Whether the
    /// copied slides stay pasteable after the throwaway presentation
    /// closes is PowerPoint's own clipboard behaviour and is part of
    /// that first check.
    fn publish_slides_to_clipboard(
        &self,
        fragment_path: &std::path::Path,
    ) -> Result<bool, String> {
        let _com = ComGuard::new()?;
        let app = match active_powerpoint() {
            Ok(a) => a,
            Err(_) => return Ok(false),
        };
        let presentations = get_obj(&app, "Presentations")?;
        // Open(FileName, ReadOnly = msoTrue, Untitled = msoFalse,
        // WithWindow = msoFalse): hidden and read-only, so closing
        // it can never prompt to save. (msoTrue is -1; this bridge
        // passes booleans as i32, as in the Add call in capture.)
        let opened = call(
            &presentations,
            "Open",
            &mut [
                VARIANT::from(BSTR::from(fragment_path.to_string_lossy().as_ref())),
                VARIANT::from(-1i32),
                VARIANT::from(0i32),
                VARIANT::from(0i32),
            ],
        )?;
        let pres = IDispatch::try_from(&opened)
            .map_err(|e| format!("打开剪贴板片段失败: {e}"))?;
        let copied = (|| -> Result<(), String> {
            let slides = get_obj(&pres, "Slides")?;
            if get_i32(&slides, "Count").unwrap_or(0) == 0 {
                return Err("剪贴板片段里没有页面".into());
            }
            // Slides.Range() with no argument selects every slide.
            let range_v = call(&slides, "Range", &mut [])?;
            let range = IDispatch::try_from(&range_v)
                .map_err(|e| format!("Slides.Range 不是对象: {e}"))?;
            call(&range, "Copy", &mut [])?;
            Ok(())
        })();
        let _ = call(&pres, "Close", &mut []);
        copied?;
        Ok(true)
    }
}
