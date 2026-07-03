//! Implémentation Windows : capture d'écran (GDI BitBlt) et injection d'entrées
//! (`SendInput`), via le crate officiel `windows`.
//!
//! Ce module n'est compilé que pour la cible Windows. Il est vérifié en
//! cross-compilation (`cargo check --target x86_64-pc-windows-gnu`) ; l'exécution
//! réelle se fait sur une machine Windows. La capture GDI est simple et robuste ;
//! une optimisation future possible est DXGI Desktop Duplication pour de
//! meilleures performances en plein écran.

use std::mem::size_of;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::{Frame, InputInjector, ScreenCapturer};
use vk_core::protocol::MouseButton;

/// Capture de l'écran principal par copie GDI (BGRA).
pub struct WindowsCapturer {
    width: i32,
    height: i32,
    screen_dc: HDC,
    mem_dc: HDC,
    bitmap: HBITMAP,
}

impl WindowsCapturer {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let width = GetSystemMetrics(SM_CXSCREEN);
            let height = GetSystemMetrics(SM_CYSCREEN);
            if width <= 0 || height <= 0 {
                anyhow::bail!("dimensions d'écran invalides ({width}x{height})");
            }
            let screen_dc = GetDC(HWND::default());
            if screen_dc.is_invalid() {
                anyhow::bail!("GetDC a échoué");
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
            SelectObject(mem_dc, HGDIOBJ(bitmap.0));
            Ok(Self {
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
            BitBlt(
                self.mem_dc,
                0,
                0,
                self.width,
                self.height,
                self.screen_dc,
                0,
                0,
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

    fn mouse_scroll(&mut self, _dx: i32, dy: i32) -> anyhow::Result<()> {
        // WHEEL_DELTA = 120 par cran ; dy positif = vers le haut.
        if dy != 0 {
            send_mouse(MOUSEEVENTF_WHEEL, 0, 0, dy * 120);
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
