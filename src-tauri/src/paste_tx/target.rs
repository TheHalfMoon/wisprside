//! Focus-bound target identity capture and synchronous pre-delivery / `WM_RENDERFORMAT`
//! re-validation (`SD-T1..SD-T6`, `delivery_outcome.schema.json`).
//!
//! Security guarantees:
//! - Captures `(window_handle, pid, start_generation, image_path_digest, integrity_level,
//!   focus_element, epoch_at_activation)` per recording session when dictation starts;
//! - Tracks external foreground window transitions via an out-of-context WinEvent hook
//!   (`EVENT_SYSTEM_FOREGROUND`) so ABA focus switches invalidate `epoch_at_activation`;
//! - Queries Windows UI Automation (`IUIAutomation::GetFocusedElement`) on an isolated
//!   MTA worker thread (`COINIT_MULTITHREADED`) with a strict `120 ms` timeout and
//!   automatic wedged-worker replacement so hung STA targets never wedge WisprSide;
//! - Refuses delivery into password fields, read-only controls (`ValuePattern.IsReadOnly`),
//!   elevated windows above our integrity level, changed foreground windows/PIDs/start
//!   generations, moved UIA focus elements inside the same HWND, unreadable `RuntimeId`s,
//!   stale epochs, or unverified Wayland sessions;
//! - Re-validates synchronously inside `WM_RENDERFORMAT` before rendering promised text.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

static FOCUS_EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn current_focus_epoch() -> u64 {
    FOCUS_EPOCH.load(Ordering::SeqCst)
}

pub fn advance_focus_epoch() -> u64 {
    FOCUS_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityLevel {
    Low,
    Medium,
    High,
    System,
    Unknown,
}

impl IntegrityLevel {
    pub fn rank(self) -> u8 {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::System => 4,
            Self::Unknown => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Decimal string of 64-bit OS process creation time (`FILETIME` on Windows).
    pub start_generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusElementSnapshot {
    pub runtime_id_digest: String,
    pub control_type: String,
    pub is_password: bool,
    pub is_read_only: bool,
    pub selected_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBinding {
    pub captured_at_activation: bool,
    pub window_handle: String,
    pub process_instance: ProcessIdentity,
    pub image_path_digest: String,
    pub executable_basename: String,
    pub integrity_level: IntegrityLevel,
    pub focus_element: Option<FocusElementSnapshot>,
    pub epoch_at_activation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRefusalReason {
    TargetChanged,
    TargetGone,
    PidReused,
    FocusLost,
    FocusMoved,
    ElevatedTarget,
    PasswordField,
    NotEditable,
    IdentityUnreadable,
    StaleEpoch,
    WaylandCopyFallback,
}

pub fn sha256_prefixed(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut hex_str = String::with_capacity(71);
    hex_str.push_str("sha256:");
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex_str, "{b:02x}");
    }
    hex_str
}

pub fn canonicalize_windows_path_for_digest(raw: &str) -> String {
    let replaced = raw.trim().replace('/', "\\");
    let stripped = replaced
        .strip_prefix(r"\\?\")
        .unwrap_or(&replaced)
        .to_lowercase();
    let is_root = stripped.len() == 3
        && stripped.as_bytes()[0].is_ascii_alphabetic()
        && &stripped[1..3] == r":\";
    if !is_root && stripped.ends_with('\\') {
        stripped[..stripped.len() - 1].to_owned()
    } else {
        stripped
    }
}

/// Pure deterministic target validation logic matching `delivery_outcome.schema.json`.
pub fn verify_binding_pair(
    activation: &TargetBinding,
    live: &TargetBinding,
    current_epoch: u64,
    our_integrity: IntegrityLevel,
) -> Result<(), DeliveryRefusalReason> {
    if activation.epoch_at_activation != current_epoch {
        return Err(DeliveryRefusalReason::StaleEpoch);
    }
    if live.window_handle == "0x0" || live.process_instance.pid == 0 {
        return Err(DeliveryRefusalReason::FocusLost);
    }
    if activation.window_handle != live.window_handle {
        return Err(DeliveryRefusalReason::TargetChanged);
    }
    if activation.process_instance.pid != live.process_instance.pid {
        return Err(DeliveryRefusalReason::TargetChanged);
    }
    if activation.process_instance.start_generation != live.process_instance.start_generation
        || activation.image_path_digest != live.image_path_digest
    {
        return Err(DeliveryRefusalReason::PidReused);
    }
    if live.integrity_level.rank() > our_integrity.rank() {
        return Err(DeliveryRefusalReason::ElevatedTarget);
    }
    let (Some(act_fe), Some(live_fe)) = (&activation.focus_element, &live.focus_element) else {
        return Err(DeliveryRefusalReason::IdentityUnreadable);
    };
    if live_fe.is_password || act_fe.is_password {
        return Err(DeliveryRefusalReason::PasswordField);
    }
    if live_fe.is_read_only || act_fe.is_read_only {
        return Err(DeliveryRefusalReason::NotEditable);
    }
    if act_fe.runtime_id_digest != live_fe.runtime_id_digest {
        return Err(DeliveryRefusalReason::FocusMoved);
    }
    Ok(())
}

/// Checks whether Linux Wayland requires explicit clipboard-copy fallback instead of blind injection.
pub fn is_unverified_wayland_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
mod win_probe {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{mpsc, Arc, Mutex, Once};
    use std::time::Duration;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
        QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern,
        SetWinEventHook, UIA_EditControlTypeId, UIA_TextPatternId, UIA_ValuePatternId,
        HWINEVENTHOOK,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
        EVENT_SYSTEM_FOREGROUND, MSG, WINEVENT_OUTOFCONTEXT,
    };

    const UIA_TIMEOUT: Duration = Duration::from_millis(120);

    static LAST_EXTERNAL_HWND: AtomicUsize = AtomicUsize::new(0);
    static FOCUS_HOOK_INIT: Once = Once::new();

    unsafe extern "system" fn foreground_event_proc(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        _id_object: i32,
        _id_child: i32,
        _id_event_thread: u32,
        _dwms_event_time: u32,
    ) {
        let hwnd_val = hwnd.0 as usize;
        if hwnd_val == 0 {
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            // Ignore WisprSide's own non-activating overlay/tray/pump windows.
            return;
        }
        let prev = LAST_EXTERNAL_HWND.swap(hwnd_val, Ordering::SeqCst);
        if prev != 0 && prev != hwnd_val {
            let _ = advance_focus_epoch();
        }
    }

    pub fn ensure_focus_epoch_watcher() {
        FOCUS_HOOK_INIT.call_once(|| {
            let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(1);
            let _ = std::thread::Builder::new()
                .name("wisprside-focus-hook".into())
                .spawn(move || unsafe {
                    let fg = GetForegroundWindow();
                    let mut fg_pid = 0u32;
                    GetWindowThreadProcessId(fg, Some(&mut fg_pid));
                    if fg.0 as usize != 0 && fg_pid != 0 && fg_pid != std::process::id() {
                        LAST_EXTERNAL_HWND.store(fg.0 as usize, Ordering::SeqCst);
                    }
                    let _hook = SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(foreground_event_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    );
                    let _ = ready_tx.send(());
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = DispatchMessageW(&msg);
                    }
                });
            let _ = ready_rx.recv_timeout(Duration::from_millis(100));
        });
    }

    struct UiaRequest {
        reply: mpsc::SyncSender<Option<FocusElementSnapshot>>,
    }

    struct UiaWorkerHandle {
        tx: mpsc::Sender<UiaRequest>,
        in_flight: Arc<AtomicBool>,
    }

    static UIA_SLOT: Mutex<Option<UiaWorkerHandle>> = Mutex::new(None);

    fn spawn_uia_worker() -> Option<UiaWorkerHandle> {
        let (tx, rx) = mpsc::channel::<UiaRequest>();
        let in_flight = Arc::new(AtomicBool::new(false));
        let worker_busy = Arc::clone(&in_flight);
        std::thread::Builder::new()
            .name("wisprside-uia-mta".into())
            .spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let uia: Option<IUIAutomation> =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
                while let Ok(req) = rx.recv() {
                    let snap = catch_unwind(AssertUnwindSafe(|| {
                        uia.as_ref().and_then(|u| query_focused_element(u))
                    }))
                    .ok()
                    .flatten();
                    worker_busy.store(false, Ordering::SeqCst);
                    let _ = req.reply.send(snap);
                }
            })
            .ok()?;
        Some(UiaWorkerHandle { tx, in_flight })
    }

    fn query_uia_with_timeout() -> Option<FocusElementSnapshot> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let send_ok = {
            let mut guard = UIA_SLOT.lock().ok()?;
            let needs_fresh = match guard.as_ref() {
                None => true,
                Some(w) => w.in_flight.load(Ordering::SeqCst),
            };
            if needs_fresh {
                *guard = spawn_uia_worker();
            }
            let worker = guard.as_ref()?;
            worker.in_flight.store(true, Ordering::SeqCst);
            if worker.tx.send(UiaRequest { reply: reply_tx }).is_ok() {
                true
            } else {
                *guard = None;
                false
            }
        };
        if !send_ok {
            return None;
        }
        reply_rx.recv_timeout(UIA_TIMEOUT).ok().flatten()
    }

    unsafe fn query_focused_element(uia: &IUIAutomation) -> Option<FocusElementSnapshot> {
        let elem = uia.GetFocusedElement().ok()?;
        let is_password = elem
            .CurrentIsPassword()
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let control_type_id = elem.CurrentControlType().unwrap_or(UIA_EditControlTypeId);
        let control_type = match control_type_id.0 {
            50004 => "Edit",
            50030 => "Document",
            50000 => "Button",
            50003 => "ComboBox",
            50020 => "Text",
            50033 => "Pane",
            _ => "Control",
        }
        .to_owned();

        // RuntimeId array -> deterministic SHA-256 digest. Fail CLOSED (`None`)
        // if GetRuntimeId fails, is null, or has unexpected SAFEARRAY dimensions/element size.
        let sa_ptr = elem.GetRuntimeId().ok()?;
        if sa_ptr.is_null() {
            return None;
        }
        let runtime_id_digest = {
            let sa = &*sa_ptr;
            let dims = sa.cDims;
            let elem_bytes = sa.cbElements;
            let data_ptr = sa.pvData as *const i32;
            let digest_opt = if dims == 1
                && elem_bytes == std::mem::size_of::<i32>() as u32
                && !data_ptr.is_null()
            {
                let count = sa.rgsabound[0].cElements as usize;
                if (1..=64).contains(&count) {
                    let ints = std::slice::from_raw_parts(data_ptr, count);
                    let mut raw = Vec::with_capacity(ints.len() * 4);
                    for &v in ints {
                        raw.extend_from_slice(&v.to_le_bytes());
                    }
                    Some(sha256_prefixed(&raw))
                } else {
                    None
                }
            } else {
                None
            };
            let _ = windows::Win32::System::Ole::SafeArrayDestroy(sa_ptr);
            digest_opt?
        };

        // Read-only detection via UIA ValuePattern (`NotEditable` enforcement).
        let is_read_only = elem
            .GetCurrentPattern(UIA_ValuePatternId)
            .ok()
            .and_then(|pat| windows::core::Interface::cast::<IUIAutomationValuePattern>(&pat).ok())
            .and_then(|vp| vp.CurrentIsReadOnly().ok())
            .map(|b| b.as_bool())
            .unwrap_or(false);

        // Non-destructive selected text extraction via UIA TextPattern (for Edit mode).
        let mut selected_text = None;
        if let Ok(pat_unk) = elem.GetCurrentPattern(UIA_TextPatternId) {
            if let Ok(text_pat) =
                windows::core::Interface::cast::<IUIAutomationTextPattern>(&pat_unk)
            {
                if let Ok(ranges) = text_pat.GetSelection() {
                    if let Ok(len) = ranges.Length() {
                        if len > 0 {
                            if let Ok(r0) = ranges.GetElement(0) {
                                if let Ok(bstr) = r0.GetText(4096) {
                                    let s = bstr.to_string();
                                    if !s.is_empty() {
                                        selected_text = Some(s);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Some(FocusElementSnapshot {
            runtime_id_digest,
            control_type,
            is_password,
            is_read_only,
            selected_text,
        })
    }

    pub fn query_process_integrity(process: windows::Win32::Foundation::HANDLE) -> IntegrityLevel {
        unsafe {
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
                return IntegrityLevel::Unknown;
            }
            let mut needed = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
            if needed == 0 || needed > 1024 {
                let _ = CloseHandle(token);
                return IntegrityLevel::Unknown;
            }
            let mut buf = vec![0u8; needed as usize];
            let ok = GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(buf.as_mut_ptr() as *mut _),
                needed,
                &mut needed,
            )
            .is_ok();
            let _ = CloseHandle(token);
            if !ok {
                return IntegrityLevel::Unknown;
            }
            let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = tml.Label.Sid;
            let count_ptr = GetSidSubAuthorityCount(sid);
            if count_ptr.is_null() || *count_ptr == 0 {
                return IntegrityLevel::Unknown;
            }
            let rid_ptr = GetSidSubAuthority(sid, (*count_ptr - 1) as u32);
            if rid_ptr.is_null() {
                return IntegrityLevel::Unknown;
            }
            match *rid_ptr {
                0x0000..=0x1FFF => IntegrityLevel::Low,
                0x2000..=0x2FFF => IntegrityLevel::Medium,
                0x3000..=0x3FFF => IntegrityLevel::High,
                _ => IntegrityLevel::System,
            }
        }
    }

    pub fn current_process_integrity() -> IntegrityLevel {
        unsafe { query_process_integrity(GetCurrentProcess()) }
    }

    /// Non-blocking Win32 process/HWND/integrity verification safe to run synchronously
    /// inside `WM_RENDERFORMAT` without cross-thread UIA COM calls.
    pub fn capture_foreground_fast(epoch: u64) -> TargetBinding {
        unsafe {
            let hwnd: HWND = GetForegroundWindow();
            let hwnd_val = hwnd.0 as usize;
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));

            if hwnd_val != 0 && pid != 0 && pid != std::process::id() {
                LAST_EXTERNAL_HWND.store(hwnd_val, Ordering::SeqCst);
            }

            let mut start_gen = "0".to_owned();
            let mut exe_path = String::new();
            let mut integrity = IntegrityLevel::Unknown;

            if pid != 0 {
                if let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut creation = FILETIME::default();
                    let mut exit = FILETIME::default();
                    let mut kernel = FILETIME::default();
                    let mut user = FILETIME::default();
                    if GetProcessTimes(proc, &mut creation, &mut exit, &mut kernel, &mut user)
                        .is_ok()
                    {
                        let ft = ((creation.dwHighDateTime as u64) << 32)
                            | (creation.dwLowDateTime as u64);
                        start_gen = ft.to_string();
                    }
                    let mut buf = vec![0u16; 1024];
                    let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(
                        proc,
                        PROCESS_NAME_WIN32,
                        PWSTR(buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok()
                    {
                        exe_path = String::from_utf16_lossy(&buf[..size as usize]);
                    }
                    integrity = query_process_integrity(proc);
                    let _ = CloseHandle(proc);
                }
            }
            let basename = std::path::Path::new(&exe_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            let canon_path = canonicalize_windows_path_for_digest(&exe_path);
            let image_path_digest = sha256_prefixed(canon_path.as_bytes());

            TargetBinding {
                captured_at_activation: false,
                window_handle: format!("0x{hwnd_val:X}"),
                process_instance: ProcessIdentity {
                    pid,
                    start_generation: start_gen,
                },
                image_path_digest,
                executable_basename: basename,
                integrity_level: integrity,
                focus_element: None,
                epoch_at_activation: epoch,
            }
        }
    }

    pub fn capture_foreground_target(epoch: u64) -> TargetBinding {
        ensure_focus_epoch_watcher();
        let mut binding = capture_foreground_fast(epoch);
        binding.captured_at_activation = true;
        binding.focus_element = query_uia_with_timeout();
        binding
    }
}

#[cfg(target_os = "windows")]
pub use win_probe::{
    capture_foreground_fast, capture_foreground_target, current_process_integrity,
};

#[cfg(not(target_os = "windows"))]
pub fn current_process_integrity() -> IntegrityLevel {
    IntegrityLevel::Medium
}

#[cfg(not(target_os = "windows"))]
pub fn capture_foreground_fast(epoch: u64) -> TargetBinding {
    capture_foreground_target(epoch)
}

#[cfg(not(target_os = "windows"))]
pub fn capture_foreground_target(epoch: u64) -> TargetBinding {
    TargetBinding {
        captured_at_activation: true,
        window_handle: "0x1".into(),
        process_instance: ProcessIdentity {
            pid: std::process::id(),
            start_generation: "1".into(),
        },
        image_path_digest: sha256_prefixed(b"posix-target"),
        executable_basename: "app".into(),
        integrity_level: IntegrityLevel::Medium,
        focus_element: if is_unverified_wayland_session() {
            None
        } else {
            Some(FocusElementSnapshot {
                runtime_id_digest: sha256_prefixed(b"default-focus"),
                control_type: "Edit".into(),
                is_password: false,
                is_read_only: false,
                selected_text: None,
            })
        },
        epoch_at_activation: epoch,
    }
}

/// Captures an immutable per-session `TargetBinding` at dictation activation.
pub fn capture_activation_target() -> TargetBinding {
    let ep = current_focus_epoch();
    capture_foreground_target(ep)
}

/// Full pre-injection validation (HWND + PID + start_generation + image digest + integrity + UIA focus + epoch).
pub fn revalidate_target_binding(activation: &TargetBinding) -> Result<(), DeliveryRefusalReason> {
    let now_ep = current_focus_epoch();
    let live = capture_foreground_target(now_ep);
    verify_binding_pair(activation, &live, now_ep, current_process_integrity())
}

/// Fast non-COM validation safe inside `WM_RENDERFORMAT` message dispatch.
pub fn verify_hwnd_process_fast(activation: &TargetBinding) -> Result<(), DeliveryRefusalReason> {
    let now_ep = current_focus_epoch();
    if activation.epoch_at_activation != now_ep {
        return Err(DeliveryRefusalReason::StaleEpoch);
    }
    let live = capture_foreground_fast(now_ep);
    if live.window_handle == "0x0" || live.process_instance.pid == 0 {
        return Err(DeliveryRefusalReason::FocusLost);
    }
    if activation.window_handle != live.window_handle
        || activation.process_instance.pid != live.process_instance.pid
    {
        return Err(DeliveryRefusalReason::TargetChanged);
    }
    if activation.process_instance.start_generation != live.process_instance.start_generation
        || activation.image_path_digest != live.image_path_digest
    {
        return Err(DeliveryRefusalReason::PidReused);
    }
    if live.integrity_level.rank() > current_process_integrity().rank() {
        return Err(DeliveryRefusalReason::ElevatedTarget);
    }
    if activation
        .focus_element
        .as_ref()
        .is_some_and(|fe| fe.is_password)
    {
        return Err(DeliveryRefusalReason::PasswordField);
    }
    if activation
        .focus_element
        .as_ref()
        .is_some_and(|fe| fe.is_read_only)
    {
        return Err(DeliveryRefusalReason::NotEditable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_binding() -> TargetBinding {
        TargetBinding {
            captured_at_activation: true,
            window_handle: "0x10A4".into(),
            process_instance: ProcessIdentity {
                pid: 4242,
                start_generation: "133700000000000000".into(),
            },
            image_path_digest: sha256_prefixed(b"c:\\program files\\code\\code.exe"),
            executable_basename: "Code.exe".into(),
            integrity_level: IntegrityLevel::Medium,
            focus_element: Some(FocusElementSnapshot {
                runtime_id_digest: sha256_prefixed(b"editor-1"),
                control_type: "Edit".into(),
                is_password: false,
                is_read_only: false,
                selected_text: Some("let x = 1;".into()),
            }),
            epoch_at_activation: 7,
        }
    }

    #[test]
    fn target_binding_enforces_stale_epoch_password_readonly_unreadable_elevation_pid_reuse_and_focus_move(
    ) {
        let act = sample_binding();
        assert_eq!(
            verify_binding_pair(&act, &act, 7, IntegrityLevel::Medium),
            Ok(())
        );

        // Stale epoch (e.g. ABA window switch) refused
        assert_eq!(
            verify_binding_pair(&act, &act, 8, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::StaleEpoch)
        );

        // Missing focus_element (unreadable RuntimeId) refused (fail-closed)
        let mut unreadable = act.clone();
        unreadable.focus_element = None;
        assert_eq!(
            verify_binding_pair(&act, &unreadable, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::IdentityUnreadable)
        );

        // Password field refused
        let mut pw = act.clone();
        pw.focus_element.as_mut().unwrap().is_password = true;
        assert_eq!(
            verify_binding_pair(&act, &pw, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::PasswordField)
        );

        // Read-only field refused
        let mut ro = act.clone();
        ro.focus_element.as_mut().unwrap().is_read_only = true;
        assert_eq!(
            verify_binding_pair(&act, &ro, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::NotEditable)
        );

        // Same HWND & PID but different UIA control inside Electron/Chromium refused
        let mut moved = act.clone();
        moved.focus_element.as_mut().unwrap().runtime_id_digest = sha256_prefixed(b"chat-box-2");
        assert_eq!(
            verify_binding_pair(&act, &moved, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::FocusMoved)
        );

        // PID reused (same PID, new start_generation) refused
        let mut reused = act.clone();
        reused.process_instance.start_generation = "133700000099999999".into();
        assert_eq!(
            verify_binding_pair(&act, &reused, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::PidReused)
        );

        // Elevated target window refused when WisprSide is Medium integrity
        let mut elev = act.clone();
        elev.integrity_level = IntegrityLevel::High;
        assert_eq!(
            verify_binding_pair(&act, &elev, 7, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::ElevatedTarget)
        );

        // Concurrent sessions stay isolated because each session owns its own TargetBinding value
        let mut session_b = sample_binding();
        session_b.window_handle = "0x20B8".into();
        session_b.epoch_at_activation = 8;
        assert_eq!(
            verify_binding_pair(&act, &session_b, 8, IntegrityLevel::Medium),
            Err(DeliveryRefusalReason::StaleEpoch)
        );
        assert_eq!(
            verify_binding_pair(&session_b, &session_b, 8, IntegrityLevel::Medium),
            Ok(())
        );
    }
}
