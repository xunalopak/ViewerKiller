//! Implémentations Windows : capture d'écran (DXGI/Desktop Duplication) et
//! injection d'entrées (`SendInput`).
//!
//! Squelette pour l'instant ; le corps réel est ajouté aux jalons 4 (capture)
//! et 5 (injection). Compile tel quel sur Windows sans dépendance externe.

use crate::{Frame, InputInjector, ScreenCapturer};
use vk_core::protocol::MouseButton;

pub struct WindowsCapturer;

impl WindowsCapturer {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::bail!("capture Windows non encore implémentée (jalon 4)")
    }
}

impl ScreenCapturer for WindowsCapturer {
    fn dimensions(&self) -> (u32, u32) {
        (0, 0)
    }
    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        Ok(None)
    }
}

pub struct WindowsInjector;

impl WindowsInjector {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::bail!("injection Windows non encore implémentée (jalon 5)")
    }
}

impl InputInjector for WindowsInjector {
    fn mouse_move(&mut self, _x: i32, _y: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn mouse_button(&mut self, _button: MouseButton, _pressed: bool) -> anyhow::Result<()> {
        Ok(())
    }
    fn mouse_scroll(&mut self, _dx: i32, _dy: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn key(&mut self, _key: u32, _pressed: bool) -> anyhow::Result<()> {
        Ok(())
    }
}
