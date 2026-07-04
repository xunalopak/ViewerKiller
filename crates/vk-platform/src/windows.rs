//! Implémentation Windows : capture d'écran (GDI BitBlt) et injection d'entrées
//! (`SendInput`), via le crate officiel `windows`.
//!
//! Ce module n'est compilé que pour la cible Windows. Il est vérifié en
//! cross-compilation (`cargo check --target x86_64-pc-windows-gnu`) ; l'exécution
//! réelle se fait sur une machine Windows. La capture GDI est simple et robuste ;
//! une optimisation future possible est DXGI Desktop Duplication pour de
//! meilleures performances en plein écran.

use std::mem::size_of;

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO,
    SRCCOPY,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::{Clipboard, Frame, InputInjector, ScreenCapturer};
use vk_core::protocol::{MonitorInfo, MouseButton};

/// Drapeau `dwFlags` marquant le moniteur principal. Non exposé nommément par le
/// crate `windows` 0.58 ; valeur stable de l'API Win32.
const MONITORINFOF_PRIMARY: u32 = 0x0000_0001;

/// Un moniteur physique en coordonnées du bureau virtuel.
struct MonitorRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    primary: bool,
}

/// Callback `EnumDisplayMonitors` : empile chaque moniteur dans le `Vec` passé
/// via `lparam`.
unsafe extern "system" fn enum_monitor(
    hmon: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<MonitorRect>);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut info).as_bool() {
        let r = info.rcMonitor;
        monitors.push(MonitorRect {
            left: r.left,
            top: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            primary: (info.dwFlags & MONITORINFOF_PRIMARY) != 0,
        });
    }
    TRUE
}

/// Énumère les moniteurs du bureau virtuel (coordonnées incluses).
fn enumerate_monitors() -> Vec<MonitorRect> {
    let mut monitors: Vec<MonitorRect> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_monitor),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }
    monitors
}

/// Crée un DC mémoire + un bitmap compatibles de la taille demandée, prêts pour
/// `BitBlt`.
unsafe fn create_target(screen_dc: HDC, width: i32, height: i32) -> (HDC, HBITMAP) {
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
    SelectObject(mem_dc, HGDIOBJ(bitmap.0));
    (mem_dc, bitmap)
}

/// Capture d'un moniteur par copie GDI (BGRA). Gère plusieurs écrans (J12) : la
/// capture copie le rectangle du moniteur sélectionné dans le bureau virtuel.
pub struct WindowsCapturer {
    monitors: Vec<MonitorRect>,
    selected: usize,
    width: i32, // dimensions du moniteur sélectionné
    height: i32,
    screen_dc: HDC,
    mem_dc: HDC,
    bitmap: HBITMAP,
}

impl WindowsCapturer {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let mut monitors = enumerate_monitors();
            if monitors.is_empty() {
                // Repli si l'énumération échoue : moniteur principal seul.
                let width = GetSystemMetrics(SM_CXSCREEN);
                let height = GetSystemMetrics(SM_CYSCREEN);
                if width <= 0 || height <= 0 {
                    anyhow::bail!("dimensions d'écran invalides ({width}x{height})");
                }
                monitors.push(MonitorRect {
                    left: 0,
                    top: 0,
                    width,
                    height,
                    primary: true,
                });
            }
            let selected = monitors.iter().position(|m| m.primary).unwrap_or(0);
            let screen_dc = GetDC(HWND::default());
            if screen_dc.is_invalid() {
                anyhow::bail!("GetDC a échoué");
            }
            let (width, height) = (monitors[selected].width, monitors[selected].height);
            let (mem_dc, bitmap) = create_target(screen_dc, width, height);
            Ok(Self {
                monitors,
                selected,
                width,
                height,
                screen_dc,
                mem_dc,
                bitmap,
            })
        }
    }
}

// Les handles GDI ne sont manipulés que depuis la tâche qui possède le capteur.
unsafe impl Send for WindowsCapturer {}

impl Drop for WindowsCapturer {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
            ReleaseDC(HWND::default(), self.screen_dc);
        }
    }
}

impl ScreenCapturer for WindowsCapturer {
    fn dimensions(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        unsafe {
            let (src_x, src_y) = {
                let m = &self.monitors[self.selected];
                (m.left, m.top)
            };
            BitBlt(
                self.mem_dc,
                0,
                0,
                self.width,
                self.height,
                self.screen_dc,
                src_x,
                src_y,
                SRCCOPY,
            )?;

            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: self.width,
                    biHeight: -self.height, // négatif = lignes top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut data = vec![0u8; (self.width * self.height * 4) as usize];
            let scanned = GetDIBits(
                self.mem_dc,
                self.bitmap,
                0,
                self.height as u32,
                Some(data.as_mut_ptr() as *mut _),
                &mut info,
                DIB_RGB_COLORS,
            );
            if scanned == 0 {
                anyhow::bail!("GetDIBits a échoué");
            }

            // GDI fournit du BGRX (octet alpha à 0) ; on force l'opacité.
            for px in data.chunks_exact_mut(4) {
                px[3] = 255;
            }

            Ok(Some(Frame {
                width: self.width as u32,
                height: self.height as u32,
                data,
            }))
        }
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        self.monitors
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                index: i as u32,
                width: m.width as u32,
                height: m.height as u32,
                primary: m.primary,
            })
            .collect()
    }

    fn select_monitor(&mut self, index: u32) -> anyhow::Result<()> {
        let idx = index as usize;
        if idx >= self.monitors.len() {
            anyhow::bail!("moniteur {index} inexistant");
        }
        if idx == self.selected {
            return Ok(());
        }
        let (width, height) = (self.monitors[idx].width, self.monitors[idx].height);
        unsafe {
            // Recrée la cible GDI à la taille du nouveau moniteur.
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
            let (mem_dc, bitmap) = create_target(self.screen_dc, width, height);
            self.mem_dc = mem_dc;
            self.bitmap = bitmap;
        }
        self.selected = idx;
        self.width = width;
        self.height = height;
        Ok(())
    }
}

/// Injection clavier/souris via `SendInput`.
pub struct WindowsInjector {
    screen_w: i32,
    screen_h: i32,
}

impl WindowsInjector {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            Ok(Self {
                screen_w: GetSystemMetrics(SM_CXSCREEN).max(1),
                screen_h: GetSystemMetrics(SM_CYSCREEN).max(1),
            })
        }
    }
}

fn send_mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
    }
}

impl InputInjector for WindowsInjector {
    fn mouse_move(&mut self, x: i32, y: i32) -> anyhow::Result<()> {
        // Coordonnées absolues normalisées sur 0..65535.
        let abs_x = (x * 65535) / (self.screen_w - 1).max(1);
        let abs_y = (y * 65535) / (self.screen_h - 1).max(1);
        send_mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, abs_x, abs_y, 0);
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> anyhow::Result<()> {
        let flag = match (button, pressed) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        send_mouse(flag, 0, 0, 0);
        Ok(())
    }

    fn mouse_scroll(&mut self, dx: i32, dy: i32) -> anyhow::Result<()> {
        // WHEEL_DELTA = 120 par cran ; dy positif = vers le haut.
        if dy != 0 {
            send_mouse(MOUSEEVENTF_WHEEL, 0, 0, dy * 120);
        }
        // Molette horizontale ; dx positif = vers la droite.
        if dx != 0 {
            send_mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx * 120);
        }
        Ok(())
    }

    fn key(&mut self, key: u32, pressed: bool) -> anyhow::Result<()> {
        let flags = if pressed {
            KEYBD_EVENT_FLAGS(0)
        } else {
            KEYEVENTF_KEYUP
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(key as u16),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
        Ok(())
    }

    fn char_input(&mut self, c: char) -> anyhow::Result<()> {
        // Frappe Unicode : wVk = 0, wScan = unité UTF-16, indépendante de la
        // disposition clavier de l'hôte. Un codepoint hors BMP produit une
        // paire de surrogates, soit deux frappes (down+up chacune).
        let mut units = [0u16; 2];
        for &unit in c.encode_utf16(&mut units).iter() {
            for keyup in [KEYBD_EVENT_FLAGS(0), KEYEVENTF_KEYUP] {
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(0),
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE | keyup,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                unsafe {
                    SendInput(&[input], size_of::<INPUT>() as i32);
                }
            }
        }
        Ok(())
    }
}

/// Presse-papiers système via `arboard`.
///
/// Sans état : un handle `arboard` est créé à la volée à chaque accès (léger
/// sous Windows, la synchro ne sonde que toutes les 500 ms). Cela garantit que
/// le type reste `Send` — il traverse les `await` d'une tâche tokio — sans
/// dépendre de la « sendabilité » du handle `arboard`. Si l'accès échoue, la
/// synchronisation est simplement sans effet.
#[derive(Default)]
pub struct WindowsClipboard;

impl WindowsClipboard {
    pub fn new() -> Self {
        Self
    }
}

impl Clipboard for WindowsClipboard {
    fn get_text(&mut self) -> Option<String> {
        let text = arboard::Clipboard::new().ok()?.get_text().ok()?;
        (!text.is_empty()).then_some(text)
    }
    fn set_text(&mut self, text: &str) {
        if let Ok(mut c) = arboard::Clipboard::new() {
            let _ = c.set_text(text.to_owned());
        }
    }
}
