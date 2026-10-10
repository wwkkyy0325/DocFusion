//! Reading a table with a model rather than with geometry.
//!
//! Everything else this project does with a page is decided by geometry: where
//! a line is, which region holds it, which column it falls in. That works for
//! prose and it does not work for tables. On the corpus the geometry gets **1
//! of 9**, and the ones it misses are the ones worth having — an eleven-column
//! financial statement, a five-column investment schedule, both of which it
//! reads as a column of loose words.
//!
//! The industry does not infer a table from whitespace either. PP-Structure is
//! **detect the lines, read them, and have a third model predict the structure
//! and the cell boxes**, and this project already has the first two — that is
//! what `rapidocr` is. What was missing is the third, and it is 7.4 MB.
//!
//! So this module does the part geometry cannot: given a picture of a table, it
//! answers with the grid. Putting the recognised words into the cells is a
//! separate step and happens in [`crate::structure`], because it needs the
//! lines and this does not.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use docfusion_core::backend::BackendError;
use ort::session::Session;
use ort::value::TensorRef;

use crate::layout;

/// The side a table is squashed into before the model sees it.
///
/// Longest side, aspect kept, then padded square to here. Both are what the
/// model's own preprocessing declares and what it was measured to want.
const SIDE: usize = 488;

/// How many tokens the model can emit. Two are its own: the first is where a
/// sequence starts and the last is where it stops.
const CLASSES: usize = 50;

/// The start of a sequence.
const START: u32 = 0;
/// The end of one.
const END: u32 = 49;

/// A cell with no span across it or down it, as one token.
///
/// **This id is not in the dictionary the model ships with, and finding that
/// out took an hour.** The model emits fifty classes and PaddleOCR's
/// `table_structure_dict.txt` has forty-eight lines, and decoding straight
/// through it turns every plain cell into ` rowspan="20"` — the file's last
/// entry — while every tag comes out right. The reason is that PaddleOCR
/// *appends* the merged form rather than storing it, so the plain cell sits
/// after the file, and the file's last entry is one it has gained since this
/// model was trained. Ids 1 through 47 are the file's first forty-seven.
const PLAIN_CELL: u32 = 48;

/// The tokens the structure can be made of, in the model's order.
///
/// The last entry of PaddleOCR's file is deliberately absent — see
/// [`PLAIN_CELL`]. Nothing here reads it and a cell that decoded to it would be
/// a cell in the wrong place.
const DICTIONARY: [&str; 47] = [
    "<thead>",
    "</thead>",
    "<tbody>",
    "</tbody>",
    "<tr>",
    "</tr>",
    "<td>",
    "<td",
    ">",
    "</td>",
    " colspan=\"2\"",
    " colspan=\"3\"",
    " colspan=\"4\"",
    " colspan=\"5\"",
    " colspan=\"6\"",
    " colspan=\"7\"",
    " colspan=\"8\"",
    " colspan=\"9\"",
    " colspan=\"10\"",
    " colspan=\"11\"",
    " colspan=\"12\"",
    " colspan=\"13\"",
    " colspan=\"14\"",
    " colspan=\"15\"",
    " colspan=\"16\"",
    " colspan=\"17\"",
    " colspan=\"18\"",
    " colspan=\"19\"",
    " colspan=\"20\"",
    " rowspan=\"2\"",
    " rowspan=\"3\"",
    " rowspan=\"4\"",
    " rowspan=\"5\"",
    " rowspan=\"6\"",
    " rowspan=\"7\"",
    " rowspan=\"8\"",
    " rowspan=\"9\"",
    " rowspan=\"10\"",
    " rowspan=\"11\"",
    " rowspan=\"12\"",
    " rowspan=\"13\"",
    " rowspan=\"14\"",
    " rowspan=\"15\"",
    " rowspan=\"16\"",
    " rowspan=\"17\"",
    " rowspan=\"18\"",
    " rowspan=\"19\"",
];

/// A cell the model predicted, by where it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// The token this cell came from, which is also the index of its box.
    pub token: usize,
    /// How many columns it covers. One, unless it spans.
    pub across: usize,
}

/// The grid the model answered with, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Grid {
    pub rows: Vec<Vec<Cell>>,
    /// One box per token, in the coordinates of the picture handed in.
    pub boxes: Vec<(f32, f32, f32, f32)>,
}

impl Grid {
    /// How many columns the widest row takes, counting what spans cover.
    pub fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.across).sum::<usize>())
            .max()
            .unwrap_or(0)
    }
}

/// The model, loaded on the first table that needs it.
pub struct Reader {
    model: PathBuf,
    session: Mutex<Option<Session>>,
}

impl Reader {
    pub fn new(model: PathBuf) -> Self {
        Self {
            model,
            session: Mutex::new(None),
        }
    }

    /// The grid in a picture of a table.
    ///
    /// `picture` is the table alone, cropped to the region the layout model
    /// found. Its boxes come back in the coordinates of that picture.
    pub fn grid(&self, picture: &image::RgbImage) -> Result<Grid, BackendError> {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if guard.is_none() {
            *guard = Some(self.load()?);
        }
        let session = guard.as_mut().expect("loaded immediately above");

        let (width, height) = (picture.width() as f32, picture.height() as f32);
        let pixels = squashed(picture);
        let shape = TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], &pixels[..]))
            .map_err(|error| BackendError::Failed(error.to_string()))?;

        let outputs = session.run(ort::inputs!["x" => shape]).map_err(|error| {
            BackendError::Failed(format!("the table model failed on this table: {error}"))
        })?;

        let (_, located) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| BackendError::Failed(error.to_string()))?;
        let (_, structure) = outputs[1]
            .try_extract_tensor::<f32>()
            .map_err(|error| BackendError::Failed(error.to_string()))?;

        let tokens = read_tokens(structure);
        let boxes = read_boxes(located, width, height);

        Ok(Grid {
            rows: rows_of(&tokens),
            boxes,
        })
    }

    fn load(&self) -> Result<Session, BackendError> {
        #[cfg_attr(not(feature = "directml"), allow(unused_mut, unused_variables))]
        let mut builder = Session::builder().map_err(|error| self.failed(error))?;

        #[cfg(feature = "directml")]
        if let Ok(session) = self.build(true) {
            return Ok(session);
        }

        self.build(false).map_err(|error| self.failed(error))
    }

    #[cfg_attr(not(feature = "directml"), allow(unused_mut, unused_variables))]
    fn build(&self, on_gpu: bool) -> ort::Result<Session> {
        let mut builder = Session::builder()?;

        #[cfg(feature = "directml")]
        if on_gpu {
            builder = builder.with_execution_providers([
                ort::ep::DirectML::default().build().error_on_failure(),
            ])?;
        }

        builder.commit_from_file(&self.model)
    }

    fn failed(&self, error: impl std::fmt::Display) -> BackendError {
        BackendError::Unavailable(format!(
            "the table model at {} could not be loaded: {error}",
            self.model.display()
        ))
    }
}

/// Whether the table model is here, checked once for the process.
///
/// Once for the same reason the layout model's answer is taken once: it decides
/// what the local engine calls itself, and the name is the cache key.
pub fn available() -> bool {
    static CHECKED: OnceLock<bool> = OnceLock::new();

    *CHECKED.get_or_init(|| layout::table_present(&layout::default_root()))
}

/// Where the model would be, whether or not it is.
pub fn model_path() -> PathBuf {
    layout::path_of(&layout::TABLE, &layout::default_root())
}

/// The table as the model wants it: bottom-up channels, longest side 488,
/// scaled and centred the way it was trained, and square.
///
/// PaddleOCR decodes to BGR and then normalises with the RGB means, which is
/// the kind of thing that looks like a mistake and is what the weights expect.
fn squashed(picture: &image::RgbImage) -> Vec<f32> {
    let (width, height) = (picture.width() as f32, picture.height() as f32);
    let scale = SIDE as f32 / width.max(height).max(1.0);
    let across = ((width * scale).round() as u32).max(1).min(SIDE as u32);
    let down = ((height * scale).round() as u32).max(1).min(SIDE as u32);

    let small = image::imageops::resize(
        picture,
        across,
        down,
        image::imageops::FilterType::Triangle,
    );

    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const DEVIATION: [f32; 3] = [0.229, 0.224, 0.225];

    let mut pixels = vec![0.0f32; 3 * SIDE * SIDE];

    for (index, pixel) in small.pixels().enumerate() {
        for channel in 0..3 {
            // Reversed, so the channel the model calls first is blue.
            let value = f32::from(pixel[2 - channel]) / 255.0;
            pixels[channel * SIDE * SIDE + index] = (value - MEAN[channel]) / DEVIATION[channel];
        }
    }

    pixels
}

/// The most likely token at each step, stopped where the model says to stop.
fn read_tokens(logits: &[f32]) -> Vec<u32> {
    logits
        .chunks_exact(CLASSES)
        .map(|step| {
            let mut best = 0;
            for (index, score) in step.iter().enumerate() {
                if *score > step[best] {
                    best = index;
                }
            }
            best as u32
        })
        .take_while(|token| *token != END)
        .filter(|token| *token != START)
        .collect()
}

/// One box per token, in the coordinates of the picture that was handed in.
///
/// **The boxes are normalised to the padded square and not to the picture.**
/// A table 1470 by 479 is resized to 488 by 159 and then padded to 488 by 488,
/// so every box lands in the top 0.326 of the square, and reading them as
/// pixels of the resized picture — which is what the first version of this did
/// — puts all of them in the top-left corner.
///
/// The sibling model `SLANeXt` answers in the resized picture instead, with the
/// same tensor signature. Two models, one family, two conventions.
fn read_boxes(located: &[f32], width: f32, height: f32) -> Vec<(f32, f32, f32, f32)> {
    let (across, down) = (width / SIDE as f32, height / SIDE as f32);

    located
        .chunks_exact(8)
        .map(|corners| {
            let mut left = f32::INFINITY;
            let mut top = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            let mut bottom = f32::NEG_INFINITY;

            for corner in corners.chunks_exact(2) {
                // Four corners, and the square they are in is not the picture.
                let x = corner[0] * SIDE as f32 * across;
                let y = corner[1] * SIDE as f32 * down;
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
            }

            (left, top, right, bottom)
        })
        .collect()
}

/// Groups the tokens into rows of cells.
///
/// A cell that covers more than one column is one cell here and is widened when
/// the row is built, because the IR has no span and a row shorter than the
/// table is a row whose later cells sit under the wrong headings.
fn rows_of(tokens: &[u32]) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Option<Vec<Cell>> = None;
    let mut across = 1;
    let mut inside = false;

    for (index, token) in tokens.iter().copied().enumerate() {
        let name = DICTIONARY.get(token.wrapping_sub(1) as usize).copied();

        match token {
            PLAIN_CELL => {
                // The merged form of a cell with no span, and one token for the
                // whole cell: its box is its own.
                if let Some(row) = row.as_mut() {
                    row.push(Cell { token: index, across: 1 });
                }
            }
            _ => match name {
                Some("<tr>") => {
                    row = Some(Vec::new());
                    across = 1;
                    inside = false;
                }
                Some("</tr>") => {
                    if let Some(taken) = row.take() {
                        rows.push(taken);
                    }
                }
                Some("<td>") | Some("<td") => {
                    across = 1;
                    inside = true;
                }
                Some("</td>") => {
                    if inside
                        && let Some(row) = row.as_mut()
                    {
                        // The box is the one this token was given, and the
                        // tokens and boxes are one apiece.
                        row.push(Cell { token: index, across });
                    }
                    inside = false;
                    across = 1;
                }
                Some(other) => {
                    if let Some(count) = span(other) {
                        across = count;
                    }
                }
                None => {}
            },
        }
    }

    if let Some(taken) = row.take() {
        rows.push(taken);
    }

    // A row with nothing in it is a wrapper the model opened and closed.
    rows.retain(|row| !row.is_empty());
    rows
}

/// How many columns or rows a span token covers.
fn span(name: &str) -> Option<usize> {
    name.rsplit('"')
        .nth(1)
        .and_then(|digits| digits.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sequence the model actually emitted for an eleven-column financial
    /// table, shortened. Every tag decoded correctly and every cell did not,
    /// which is how the dictionary mismatch was found.
    #[test]
    fn a_plain_cell_is_its_own_token_and_not_in_the_dictionary() {
        assert_eq!(PLAIN_CELL as usize, DICTIONARY.len() + 1);
        assert!(!DICTIONARY.contains(&"<td></td>"));

        let tokens = [5, PLAIN_CELL, PLAIN_CELL, PLAIN_CELL, 6];
        let rows = rows_of(&tokens);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[0][0].token, 1);
    }

    #[test]
    fn the_dictionary_starts_where_the_model_says_it_does() {
        assert_eq!(DICTIONARY[0], "<thead>");
        assert_eq!(DICTIONARY[4], "<tr>");
        assert_eq!(DICTIONARY[5], "</tr>");
    }

    #[test]
    fn a_wrapped_table_becomes_its_rows() {
        let header = [1u32, 5, PLAIN_CELL, PLAIN_CELL, 6, 2];
        let body = [5, PLAIN_CELL, PLAIN_CELL, 6];
        let mut all = header.to_vec();
        all.extend_from_slice(&body);

        let rows = rows_of(&all);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[1].len(), 2);
    }

    /// A cell that covers two columns is one cell here and is widened later,
    /// because a row shorter than the table puts its later cells under the
    /// wrong headings.
    #[test]
    fn a_spanned_cell_carries_its_width() {
        let tokens = [5, 8, 11, 9, 10, PLAIN_CELL, 6];
        let rows = rows_of(&tokens);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[0][0].across, 2);
        assert_eq!(rows[0][1].across, 1);
    }

    #[test]
    fn the_widest_row_says_how_wide_the_table_is() {
        let grid = Grid {
            rows: rows_of(&[5, PLAIN_CELL, PLAIN_CELL, 6, 5, 8, 11, 9, 10, PLAIN_CELL, 6]),
            boxes: Vec::new(),
        };

        assert_eq!(grid.columns(), 3);
    }

    /// The boxes are normalised to the padded square, not to the picture, and
    /// the difference is a whole table in the top-left corner.
    #[test]
    fn boxes_are_mapped_from_the_square_to_the_picture() {
        let located = [0.0, 0.0, 0.5, 0.0, 0.5, 0.25, 0.0, 0.25];

        let boxes = read_boxes(&located, 1470.0, 479.0);

        assert_eq!(boxes.len(), 1);
        // Half way across the square is half way across the picture, but a
        // quarter of the way down a square that is 488 tall is a quarter of
        // the picture's height only because the box was mapped through the
        // square's own side first.
        let (left, top, right, bottom) = boxes[0];
        assert!((left - 0.0).abs() < 1e-3);
        assert!((right - 735.0).abs() < 1.0, "{right}");
        assert!((top - 0.0).abs() < 1e-3);
        assert!((bottom - 119.75).abs() < 1.0, "{bottom}");
    }

    #[test]
    fn a_sequence_stops_where_the_model_stops() {
        let mut logits = Vec::new();
        for token in [START, 5u32, PLAIN_CELL, END, 5] {
            let mut step = vec![0.0f32; CLASSES];
            step[token as usize] = 1.0;
            logits.extend_from_slice(&step);
        }

        assert_eq!(read_tokens(&logits), vec![5, PLAIN_CELL]);
    }

    #[test]
    fn a_span_token_is_read_as_a_number() {
        assert_eq!(span(" colspan=\"2\""), Some(2));
        assert_eq!(span(" rowspan=\"17\""), Some(17));
        assert_eq!(span("<tr>"), None);
    }

    /// The picture the model is handed is square, normalised, and in the order
    /// the weights expect.
    #[test]
    fn a_wide_table_is_squashed_into_a_square() {
        let picture = image::RgbImage::from_pixel(1470, 479, image::Rgb([255, 255, 255]));

        let pixels = squashed(&picture);

        assert_eq!(pixels.len(), 3 * SIDE * SIDE);
        // White is one before the shift, and the shift is not zero, so a
        // channel is either the top or the bottom of the picture.
        let top = pixels[0];
        let below = pixels[SIDE * (SIDE / 2)];
        assert!(top > 1.5, "{top}");
        assert_eq!(below, 0.0, "past the picture is padding");
    }
}
