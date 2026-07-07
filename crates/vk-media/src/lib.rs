//! Encodage et décodage des trames écran de ViewerKiller.
//!
//! Côté **hôte** : [`TileEncoder`] compare la trame courante à la précédente,
//! découpe l'écran en tuiles et n'encode (JPEG) que celles qui ont changé,
//! produisant un [`FrameUpdate`] compact.
//!
//! Côté **contrôleur** : [`FrameBuffer`] applique ces mises à jour sur un tampon
//! RGBA prêt à être affiché (par ex. comme texture egui).
//!
//! Tout est en Rust pur (jpeg-encoder/jpeg-decoder/miniz_oxide) : aucune
//! dépendance système, donc compilable et testable sur n'importe quelle
//! plateforme.

use std::time::Duration;

use vk_core::protocol::{FrameUpdate, Tile, TileCodec};
use vk_platform::Frame;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("encodage JPEG : {0}")]
    JpegEncode(#[from] jpeg_encoder::EncodingError),
    #[error("décodage JPEG : {0}")]
    JpegDecode(#[from] jpeg_decoder::Error),
    #[error("décompression deflate échouée")]
    Inflate,
    #[error("tuile ou trame incohérente")]
    BadTile,
}

/// Taille de tuile par défaut (en pixels).
pub const DEFAULT_TILE_SIZE: u32 = 128;
/// Qualité JPEG par défaut (0–100).
pub const DEFAULT_QUALITY: u8 = 75;

/// Qualité JPEG minimale sous forte contrainte (J10b) : en dessous, l'image
/// devient inexploitable ; on préfère baisser le débit d'images.
pub const MIN_QUALITY: u8 = 30;

/// Régulateur de qualité adaptatif (J10b) : ajuste la qualité JPEG selon le temps
/// réel de traitement d'une trame (capture + encodage + envoi) comparé à la
/// période cible. Si le cycle **déborde** (réseau lent, backpressure), la qualité
/// baisse pour réduire le débit ; si la marge est **confortable**, elle remonte
/// progressivement vers le maximum configuré. Hystérésis pour éviter les
/// oscillations. Logique pure, testable sans réseau.
#[derive(Debug, Clone)]
pub struct QualityController {
    current: u8,
    max: u8,
    min: u8,
    /// Nombre de cycles confortables consécutifs (avant de remonter la qualité).
    good_streak: u32,
}

impl QualityController {
    /// `max` = qualité configurée (plafond ; aussi la valeur de départ).
    pub fn new(max: u8) -> Self {
        Self {
            current: max,
            max,
            min: MIN_QUALITY.min(max),
            good_streak: 0,
        }
    }

    /// Qualité JPEG courante à appliquer à l'encodeur.
    pub fn quality(&self) -> u8 {
        self.current
    }

    /// Met à jour la qualité d'après le temps de cycle `cycle` d'une trame et la
    /// période cible `period` (= 1/fps).
    pub fn observe(&mut self, cycle: Duration, period: Duration) {
        if cycle > period.mul_f64(1.3) {
            // Débordement franc : on baisse vite pour dégager du débit.
            self.current = self.current.saturating_sub(8).max(self.min);
            self.good_streak = 0;
        } else if cycle < period.mul_f64(0.6) {
            // Large marge : on remonte doucement (après plusieurs bons cycles).
            self.good_streak += 1;
            if self.good_streak >= 10 {
                self.current = (self.current + 3).min(self.max);
                self.good_streak = 0;
            }
        } else {
            self.good_streak = 0;
        }
    }
}

/// Encodeur incrémental : n'émet que les tuiles modifiées entre deux trames.
pub struct TileEncoder {
    tile_size: u32,
    quality: u8,
    width: u32,
    height: u32,
    prev: Option<Vec<u8>>,
    seq: u64,
}

impl TileEncoder {
    pub fn new(tile_size: u32, quality: u8) -> Self {
        Self {
            tile_size: tile_size.max(8),
            quality,
            width: 0,
            height: 0,
            prev: None,
            seq: 0,
        }
    }

    /// Force la prochaine trame à être émise intégralement (toutes les tuiles).
    pub fn force_full_frame(&mut self) {
        self.prev = None;
    }

    /// Ajuste la qualité JPEG à chaud (cadence/qualité adaptatives, J10b).
    pub fn set_quality(&mut self, quality: u8) {
        self.quality = quality;
    }

    /// Encode `frame` en ne conservant que les tuiles modifiées.
    pub fn encode(&mut self, frame: &Frame) -> Result<FrameUpdate, MediaError> {
        if !frame.is_well_formed() {
            return Err(MediaError::BadTile);
        }
        if frame.width != self.width || frame.height != self.height {
            self.width = frame.width;
            self.height = frame.height;
            self.prev = None;
        }

        let (w, h, ts) = (frame.width, frame.height, self.tile_size);
        let mut tiles = Vec::new();
        let mut ty = 0;
        while ty < h {
            let th = ts.min(h - ty);
            let mut tx = 0;
            while tx < w {
                let tw = ts.min(w - tx);
                let changed = match &self.prev {
                    Some(prev) => tile_differs(&frame.data, prev, w, tx, ty, tw, th),
                    None => true,
                };
                if changed {
                    let bgra = extract_tile(&frame.data, w, tx, ty, tw, th);
                    let data = encode_jpeg(&bgra, tw, th, self.quality)?;
                    tiles.push(Tile {
                        x: tx,
                        y: ty,
                        width: tw,
                        height: th,
                        codec: TileCodec::Jpeg,
                        data,
                    });
                }
                tx += ts;
            }
            ty += ts;
        }

        self.prev = Some(frame.data.clone());
        self.seq += 1;
        Ok(FrameUpdate {
            seq: self.seq,
            tiles,
        })
    }
}

/// Tampon RGBA reconstitué côté contrôleur, prêt pour l'affichage.
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// Pixels RGBA (4 octets/pixel), row-major.
    pub rgba: Vec<u8>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0u8; (width as usize) * (height as usize) * 4],
        }
    }

    /// Redimensionne (et réinitialise) le tampon.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    }

    /// Applique une mise à jour : décode chaque tuile et la recopie en place.
    pub fn apply(&mut self, update: &FrameUpdate) -> Result<(), MediaError> {
        for tile in &update.tiles {
            let rgba = decode_tile(tile)?;
            self.blit(&rgba, tile.x, tile.y, tile.width, tile.height)?;
        }
        Ok(())
    }

    fn blit(
        &mut self,
        rgba_tile: &[u8],
        x: u32,
        y: u32,
        tw: u32,
        th: u32,
    ) -> Result<(), MediaError> {
        if x + tw > self.width || y + th > self.height {
            return Err(MediaError::BadTile);
        }
        if rgba_tile.len() != (tw as usize) * (th as usize) * 4 {
            return Err(MediaError::BadTile);
        }
        let stride = self.width as usize * 4;
        let row_bytes = tw as usize * 4;
        for row in 0..th as usize {
            let dst = (y as usize + row) * stride + x as usize * 4;
            let src = row * row_bytes;
            self.rgba[dst..dst + row_bytes].copy_from_slice(&rgba_tile[src..src + row_bytes]);
        }
        Ok(())
    }
}

// --- Fonctions internes ----------------------------------------------------

fn extract_tile(data: &[u8], width: u32, tx: u32, ty: u32, tw: u32, th: u32) -> Vec<u8> {
    let stride = width as usize * 4;
    let row_bytes = tw as usize * 4;
    let mut out = Vec::with_capacity(th as usize * row_bytes);
    for row in 0..th as usize {
        let start = (ty as usize + row) * stride + tx as usize * 4;
        out.extend_from_slice(&data[start..start + row_bytes]);
    }
    out
}

fn tile_differs(cur: &[u8], prev: &[u8], width: u32, tx: u32, ty: u32, tw: u32, th: u32) -> bool {
    if cur.len() != prev.len() {
        return true;
    }
    let stride = width as usize * 4;
    let row_bytes = tw as usize * 4;
    for row in 0..th as usize {
        let start = (ty as usize + row) * stride + tx as usize * 4;
        if cur[start..start + row_bytes] != prev[start..start + row_bytes] {
            return true;
        }
    }
    false
}

fn encode_jpeg(bgra: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>, MediaError> {
    let rgb = bgra_to_rgb(bgra);
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality).encode(
        &rgb,
        w as u16,
        h as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

fn decode_tile(tile: &Tile) -> Result<Vec<u8>, MediaError> {
    match tile.codec {
        TileCodec::Jpeg => {
            let (rgba, w, h) = decode_jpeg(&tile.data)?;
            if w != tile.width || h != tile.height {
                return Err(MediaError::BadTile);
            }
            Ok(rgba)
        }
        TileCodec::DeflateBgra => {
            let bgra = miniz_oxide::inflate::decompress_to_vec(&tile.data)
                .map_err(|_| MediaError::Inflate)?;
            if bgra.len() != (tile.width as usize) * (tile.height as usize) * 4 {
                return Err(MediaError::BadTile);
            }
            Ok(bgra_to_rgba(&bgra))
        }
    }
}

fn decode_jpeg(data: &[u8]) -> Result<(Vec<u8>, u32, u32), MediaError> {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    let pixels = dec.decode()?;
    let info = dec.info().ok_or(MediaError::BadTile)?;
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => rgb_to_rgba(&pixels),
        jpeg_decoder::PixelFormat::L8 => l8_to_rgba(&pixels),
        _ => return Err(MediaError::BadTile),
    };
    Ok((rgba, info.width as u32, info.height as u32))
}

fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
    for px in bgra.chunks_exact(4) {
        rgb.push(px[2]); // R
        rgb.push(px[1]); // G
        rgb.push(px[0]); // B
    }
    rgb
}

fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        rgba.push(px[2]); // R
        rgba.push(px[1]); // G
        rgba.push(px[0]); // B
        rgba.push(px[3]); // A
    }
    rgba
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        rgba.extend_from_slice(px);
        rgba.push(255);
    }
    rgba
}

fn l8_to_rgba(l: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(l.len() * 4);
    for &v in l {
        rgba.extend_from_slice(&[v, v, v, 255]);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_drops_under_overrun_and_recovers() {
        let period = Duration::from_millis(66); // ~15 fps
        let mut q = QualityController::new(75);
        assert_eq!(q.quality(), 75);
        // Débordement répété → la qualité baisse.
        for _ in 0..3 {
            q.observe(Duration::from_millis(200), period);
        }
        let low = q.quality();
        assert!(low < 75, "la qualité aurait dû baisser (={low})");
        // Marge confortable prolongée → la qualité remonte.
        for _ in 0..40 {
            q.observe(Duration::from_millis(10), period);
        }
        assert!(q.quality() > low, "la qualité aurait dû remonter");
    }

    #[test]
    fn quality_stays_within_bounds() {
        let period = Duration::from_millis(66);
        let mut q = QualityController::new(75);
        for _ in 0..50 {
            q.observe(Duration::from_millis(500), period);
        }
        assert!(
            q.quality() >= MIN_QUALITY,
            "ne descend pas sous le plancher"
        );
        for _ in 0..500 {
            q.observe(Duration::from_millis(1), period);
        }
        assert_eq!(q.quality(), 75, "ne dépasse pas le plafond configuré");
    }

    fn solid_frame(width: u32, height: u32, bgra: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&bgra);
        }
        Frame {
            width,
            height,
            data,
        }
    }

    #[test]
    fn solid_color_round_trips_within_tolerance() {
        // B=10, G=20, R=30, A=255
        let frame = solid_frame(96, 96, [10, 20, 30, 255]);
        let mut enc = TileEncoder::new(64, 90);
        let update = enc.encode(&frame).unwrap();
        assert!(!update.tiles.is_empty());

        let mut fb = FrameBuffer::new(96, 96);
        fb.apply(&update).unwrap();

        // pixel central, attendu RGBA = (30, 20, 10, 255)
        let idx = ((48 * 96) + 48) * 4;
        let px = &fb.rgba[idx..idx + 4];
        assert!((px[0] as i32 - 30).abs() <= 4, "R={}", px[0]);
        assert!((px[1] as i32 - 20).abs() <= 4, "G={}", px[1]);
        assert!((px[2] as i32 - 10).abs() <= 4, "B={}", px[2]);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn unchanged_frame_emits_no_tiles() {
        let frame = solid_frame(128, 128, [0, 0, 0, 255]);
        let mut enc = TileEncoder::new(64, 75);
        let first = enc.encode(&frame).unwrap();
        assert_eq!(first.tiles.len(), 4); // 128/64 = 2x2 tuiles
        let second = enc.encode(&frame).unwrap();
        assert!(second.tiles.is_empty());
    }

    #[test]
    fn only_changed_tile_is_re_emitted() {
        let frame = solid_frame(128, 128, [0, 0, 0, 255]);
        let mut enc = TileEncoder::new(64, 75);
        let _ = enc.encode(&frame).unwrap();

        // modifie un pixel dans la tuile en bas à droite (tuile 1,1)
        let mut frame2 = frame.clone();
        let idx = ((100 * 128) + 100) * 4;
        frame2.data[idx] = 255;
        let update = enc.encode(&frame2).unwrap();
        assert_eq!(update.tiles.len(), 1);
        assert_eq!(update.tiles[0].x, 64);
        assert_eq!(update.tiles[0].y, 64);
    }

    #[test]
    fn non_divisible_dimensions_cover_full_surface() {
        // 100x70 avec tuiles de 64 → tailles 64 et 36 (largeur), 64 et 6 (hauteur)
        let frame = solid_frame(100, 70, [5, 5, 5, 255]);
        let mut enc = TileEncoder::new(64, 75);
        let update = enc.encode(&frame).unwrap();
        // 2 colonnes x 2 lignes = 4 tuiles
        assert_eq!(update.tiles.len(), 4);
        let covered: u32 = update.tiles.iter().map(|t| t.width * t.height).sum();
        assert_eq!(covered, 100 * 70);
    }

    #[test]
    fn deflate_tile_decodes() {
        // construit une tuile 2x2 BGRA compressée par deflate
        let bgra: Vec<u8> = vec![
            10, 20, 30, 255, // px0
            40, 50, 60, 255, // px1
            70, 80, 90, 255, // px2
            100, 110, 120, 255, // px3
        ];
        let compressed = miniz_oxide::deflate::compress_to_vec(&bgra, 6);
        let tile = Tile {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            codec: TileCodec::DeflateBgra,
            data: compressed,
        };
        let mut fb = FrameBuffer::new(2, 2);
        fb.apply(&FrameUpdate {
            seq: 1,
            tiles: vec![tile],
        })
        .unwrap();
        // px0 BGRA (10,20,30) → RGBA (30,20,10,255)
        assert_eq!(&fb.rgba[0..4], &[30, 20, 10, 255]);
        // px3 BGRA (100,110,120) → RGBA (120,110,100,255)
        assert_eq!(&fb.rgba[12..16], &[120, 110, 100, 255]);
    }
}
