//! Thin UI Automation wrapper over the `windows` crate.

use anyhow::Result;
use windows::core::BSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationValuePattern, TreeScope_Subtree, UIA_InvokePatternId, UIA_ValuePatternId,
};

// Common control-type ids (UIA_CONTROLTYPE_ID values).
pub const CT_BUTTON: i32 = 50000;
pub const CT_EDIT: i32 = 50004;
pub const CT_TEXT: i32 = 50020;
pub const CT_DOCUMENT: i32 = 50030;
pub const CT_LISTITEM: i32 = 50007;

pub struct Uia {
    pub automation: IUIAutomation,
}

impl Uia {
    pub fn new() -> Result<Self> {
        unsafe {
            // S_FALSE if COM already initialised on this thread — harmless.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL)?;
            Ok(Self { automation })
        }
    }

    pub fn element_from_hwnd(&self, hwnd: HWND) -> Result<IUIAutomationElement> {
        unsafe { Ok(self.automation.ElementFromHandle(hwnd)?) }
    }

    /// Flatten the whole UIA subtree under `root` into a Vec.
    pub fn subtree(&self, root: &IUIAutomationElement) -> Result<Vec<IUIAutomationElement>> {
        unsafe {
            let cond = self.automation.CreateTrueCondition()?;
            let arr = root.FindAll(TreeScope_Subtree, &cond)?;
            let len = arr.Length()?;
            let mut out = Vec::with_capacity(len as usize);
            for i in 0..len {
                out.push(arr.GetElement(i)?);
            }
            Ok(out)
        }
    }
}

pub fn name(el: &IUIAutomationElement) -> String {
    unsafe { el.CurrentName().map(|b| b.to_string()).unwrap_or_default() }
}

pub fn control_type(el: &IUIAutomationElement) -> i32 {
    unsafe { el.CurrentControlType().map(|c| c.0).unwrap_or(0) }
}

pub fn current_value(el: &IUIAutomationElement) -> String {
    unsafe {
        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()
            .and_then(|vp| vp.CurrentValue().ok())
            .map(|b| b.to_string())
            .unwrap_or_default()
    }
}

pub fn set_focus(el: &IUIAutomationElement) {
    unsafe {
        let _ = el.SetFocus();
    }
}

/// Screen bounding rectangle (left, top, right, bottom) of an element.
pub fn bounding_rect(el: &IUIAutomationElement) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        el.CurrentBoundingRectangle()
            .ok()
            .map(|r| (r.left, r.top, r.right, r.bottom))
    }
}

pub fn has_value_pattern(el: &IUIAutomationElement) -> bool {
    unsafe {
        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .is_ok()
    }
}

/// Type `text` into a ValuePattern-capable element (e.g. a chat input box).
pub fn set_value(el: &IUIAutomationElement, text: &str) -> Result<()> {
    unsafe {
        let vp: IUIAutomationValuePattern = el.GetCurrentPatternAs(UIA_ValuePatternId)?;
        vp.SetValue(&BSTR::from(text))?;
        Ok(())
    }
}

/// Invoke (click) an InvokePattern-capable element (e.g. a send button).
pub fn invoke(el: &IUIAutomationElement) -> Result<()> {
    unsafe {
        let ip: IUIAutomationInvokePattern = el.GetCurrentPatternAs(UIA_InvokePatternId)?;
        ip.Invoke()?;
        Ok(())
    }
}

pub fn control_type_name(ct: i32) -> &'static str {
    match ct {
        CT_BUTTON => "Button",
        CT_EDIT => "Edit",
        CT_TEXT => "Text",
        CT_DOCUMENT => "Document",
        CT_LISTITEM => "ListItem",
        _ => "?",
    }
}
