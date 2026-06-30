//! Implémentations factices pour le développement hors Windows.
//!
//! Le capteur génère une trame BGRA unie dont la teinte évolue, ce qui suffit à
//! valider la chaîne réseau et le rendu de l'UI sans matériel Windows.
//! L'injecteur ignore silencieusement les événements.

use crate::{Frame, InputInjector, ScreenCapturer};
use vk_core::protocol::MouseButton;

pub struct StubCapturer {
    width: u32,
    height: u32,
    tick: u64,
}

impl StubCapturer {
    pub fn new() -> Self {
        Self {
            width: 640,
            height: 480,
            tick: 0,
        }
    }
}

impl Default for StubCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenCapturer for StubCapturer {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        self.tick = self.tick.wrapping_add(1);
        let v = (self.tick % 256) as u8;
        let data = vec![v; (self.width as usize) * (self.height as usize) * 4];
        Ok(Some(Frame {
            width: self.width,
            height: self.height,
            data,
        }))
    }
}

pub struct StubInjector;

impl InputInjector for StubInjector {
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
