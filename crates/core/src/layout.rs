//! Global nested squarified treemap (Finviz-style). Sectors packed 2D, stocks
//! packed 2D within each sector box.
//!
//! Sector boxes use sqrt(total cap), NOT raw total: raw would make the biggest
//! sector ~18x the smallest, crushing small sectors (e.g. utilities) until their
//! stocks vanish. sqrt compresses that to ~4x — big sectors still clearly bigger,
//! small sectors stay big enough to read. Stocks WITHIN a box stay linear (raw
//! cap), so within-sector size is faithful.
//! Geometry depends only on caps + canvas size (NOT timeframe) -> computed once.

use crate::data::Sector;
use treemap::{Mappable, Rect, TreemapLayout};

/// A laid-out rectangle in pixels, in CANVAS coordinates.
#[derive(Clone, Copy, Debug)]
pub struct TileRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl From<&Rect> for TileRect {
    fn from(r: &Rect) -> Self {
        TileRect { x: r.x, y: r.y, w: r.w, h: r.h }
    }
}

pub struct StockTile {
    pub symbol: &'static str,
    pub market_cap: f64,
    pub closes: Vec<f64>,
    pub rect: TileRect,
}

pub struct SectorBox {
    pub name: &'static str,
    pub rect: TileRect,
    pub tiles: Vec<StockTile>,
}

/// A Mappable that remembers its original index (layout_items sorts the slice).
struct Cell {
    id: usize,
    size: f64,
    bounds: Rect,
}

impl Mappable for Cell {
    fn size(&self) -> f64 {
        self.size
    }
    fn bounds(&self) -> &Rect {
        &self.bounds
    }
    fn set_bounds(&mut self, b: Rect) {
        self.bounds = b;
    }
}

/// Squarify `sizes` inside `area`; return (original index, rect).
fn squarify(sizes: impl Iterator<Item = f64>, area: Rect) -> Vec<(usize, Rect)> {
    let mut cells: Vec<Cell> = sizes
        .enumerate()
        .map(|(id, size)| Cell { id, size, bounds: Rect::new() })
        .collect();
    TreemapLayout::new().layout_items(&mut cells, area);
    cells.iter().map(|c| (c.id, *c.bounds())).collect()
}

/// Two-level layout. `gap` = inset inside each sector box; `header` = strip
/// reserved at the top of each sector box for its label.
pub fn layout(
    sectors: Vec<Sector>,
    width: f64,
    height: f64,
    gap: f64,
    header: f64,
) -> Vec<SectorBox> {
    let sector_sizes = sectors
        .iter()
        // sqrt(total) so small sectors don't get crushed to invisibility
        .map(|s| s.stocks.iter().map(|st| st.market_cap).sum::<f64>().sqrt());
    let placed = squarify(sector_sizes, Rect::from_points(0.0, 0.0, width, height));

    let mut out = Vec::with_capacity(sectors.len());
    for (sid, brect) in placed {
        let sector = &sectors[sid];
        // shrink for the gap + reserve the header strip, then pack stocks inside
        let inner = Rect::from_points(
            brect.x + gap,
            brect.y + gap + header,
            (brect.w - 2.0 * gap).max(0.0),
            (brect.h - 2.0 * gap - header).max(0.0),
        );
        let placed_stocks = squarify(sector.stocks.iter().map(|st| st.market_cap), inner);
        let tiles = placed_stocks
            .into_iter()
            .map(|(j, r)| {
                let st = &sector.stocks[j];
                StockTile {
                    symbol: st.symbol,
                    market_cap: st.market_cap,
                    closes: st.closes.clone(),
                    rect: (&r).into(),
                }
            })
            .collect();
        out.push(SectorBox { name: sector.name, rect: (&brect).into(), tiles });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::mock_sectors;

    #[test]
    fn tiles_fit_and_count_matches() {
        let (w, h) = (1000.0, 680.0);
        let boxes = layout(mock_sectors(), w, h, 4.0, 18.0);
        assert_eq!(boxes.len(), 10);
        let mut n = 0;
        for b in &boxes {
            for t in &b.tiles {
                n += 1;
                assert!(t.rect.x >= -0.01 && t.rect.y >= -0.01);
                assert!(t.rect.x + t.rect.w <= w + 0.01, "overflow width: {:?}", t.rect);
                assert!(t.rect.y + t.rect.h <= h + 0.01, "overflow height: {:?}", t.rect);
            }
        }
        assert_eq!(n, 100);
    }
}
