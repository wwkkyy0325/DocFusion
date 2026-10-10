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
use std::sync::Mutex;

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
    /// How many columns it covers. One, unless it spans across.
    pub across: usize,
    /// How many rows it covers. One, unless it spans down.
    ///
    /// Kept apart from `across` because the model has a token for each and they
    /// are not the same thing: `rowspan="3"` is a cell three rows tall, and a
    /// table whose merged label was read as three columns wide comes out with
    /// every column after it in the wrong place and every value in the wrong
    /// cell. Measured that way on an equipment register with one merged label.
    pub down: usize,
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
        let (pixels, shaped) = squashed(picture);
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
        let boxes = read_boxes(located, shaped, width, height);

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
/// what the local engine calls itself, and the name is the cache key. And
/// forgotten for the same reason, by [`forget`].
pub fn available() -> bool {
    AVAILABLE.get(|| layout::table_present(&layout::default_root()))
}

/// Forgets what was found, so the next question looks at the disk again.
///
/// Called after a fetch that wrote the model. See [`layout::Cached`] for why
/// this is not a `OnceLock`.
pub fn forget() {
    AVAILABLE.forget()
}

/// The answer to [`available`], once it has been asked and until it is
/// forgotten.
static AVAILABLE: layout::Cached = layout::Cached::new();

/// Where the model would be, whether or not it is.
pub fn model_path() -> PathBuf {
    layout::path_of(&layout::TABLE, &layout::default_root())
}

/// The table as the model wants it: bottom-up channels, longest side 488,
/// scaled and centred the way it was trained, and square.
///
/// PaddleOCR decodes to BGR and then normalises with the RGB means, which is
/// the kind of thing that looks like a mistake and is what the weights expect.
///
/// Returns the pixels and the size the picture was given — the long side 488
/// and the other side shorter — because the boxes come back against the square
/// and that size is the only thing that says where the picture sat inside it.
fn squashed(picture: &image::RgbImage) -> (Vec<f32>, (f32, f32)) {
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

    // Written a row at a time into a square that is `SIDE` wide, which is what
    // the model reads. Walking `small`'s own pixels instead would use its width
    // as the stride: for a picture whose long side is its height the two differ,
    // and the image is then laid down sheared — each line of the square carrying
    // a line and a half of the picture — which the model answers with a correct
    // row count and boxes that belong to nothing. Every table in the corpus was
    // wider than it was tall, where the two strides are equal and the bug has
    // nothing to do.
    for (y, row) in small.rows().enumerate() {
        for (x, pixel) in row.enumerate() {
            let at = y * SIDE + x;

            for channel in 0..3 {
                // Reversed, so the channel the model calls first is blue.
                let value = f32::from(pixel[2 - channel]) / 255.0;
                pixels[channel * SIDE * SIDE + at] = (value - MEAN[channel]) / DEVIATION[channel];
            }
        }
    }

    (pixels, (across as f32, down as f32))
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
/// **Getting from the square to the picture is two steps, not one.** The box
/// times the square's side is where it is in the square; that times the picture
/// over the resized size is where it is in the picture. Doing only the first
/// and then scaling by the picture, which is what this did second, is right
/// when the two sides happen to be equal — a wide table is resized to 488 by
/// 148, and a table of fifteen rows came out sixty pixels tall, every row of it
/// a ten-pixel stripe with its words stacked in rows that were not its own.
///
/// The sibling model `SLANeXt` answers in the resized picture instead, with the
/// same tensor signature. Two models, one family, two conventions.
fn read_boxes(
    located: &[f32],
    resized: (f32, f32),
    width: f32,
    height: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let (across, down) = (width / resized.0, height / resized.1);

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
    let mut down = 1;
    let mut inside = false;
    // Where the cell now open began, for a cell the model never closes.
    let mut opened: Option<usize> = None;

    for (index, token) in tokens.iter().copied().enumerate() {
        let name = DICTIONARY.get(token.wrapping_sub(1) as usize).copied();

        match token {
            PLAIN_CELL => {
                // The merged form of a cell with no span, and one token for the
                // whole cell: its box is its own.
                //
                // A cell left open is deliberately **not** finished here, even
                // though a plain cell is plainly a new one. Finishing it puts
                // its position before the words that follow, and the words are
                // what the page has: measured on the register, closing it here
                // moved every value in the row one column to the right and put
                // an empty column where the count should be. Left open, it is
                // finished at the end of the row and its position falls after
                // them, which costs an empty column at the edge and nothing
                // else.
                if let Some(row) = row.as_mut() {
                    row.push(Cell {
                        token: index,
                        across: 1,
                        down: 1,
                    });
                }
            }
            _ => match name {
                Some("<tr>") => {
                    row = Some(Vec::new());
                    across = 1;
                    down = 1;
                    inside = false;
                    opened = None;
                }
                Some("</tr>") => {
                    // A cell still open here was never closed either.
                    if inside
                        && let (Some(row), Some(opened)) = (row.as_mut(), opened)
                    {
                        row.push(Cell {
                            token: opened,
                            across,
                            down,
                        });
                    }

                    inside = false;
                    opened = None;
                    across = 1;
                    down = 1;

                    if let Some(taken) = row.take() {
                        rows.push(taken);
                    }
                }
                Some("<td>") | Some("<td") => {
                    // A cell the model never closed. It emits
                    // `<td> rowspan="2" <td >` when it means one tall cell —
                    // seen on an equipment register with a merged label — and a
                    // reader that only finishes a cell at `</td>` drops that
                    // cell, and the merge with it, entirely. A new `<td` is
                    // where the one before it ended.
                    //
                    // The box it is given is the one from where it **opened**
                    // and not from the token that replaced it. Measured on that
                    // register: the token that opened the tall cell carried the
                    // box of the whole merged label, and the `<td` that followed
                    // carried the box of something else, so closing onto the
                    // later one left the label with no cell to land in.
                    if inside
                        && let (Some(row), Some(opened)) = (row.as_mut(), opened)
                    {
                        row.push(Cell {
                            token: opened,
                            across,
                            down,
                        });
                    }

                    across = 1;
                    down = 1;
                    inside = true;
                    opened = Some(index);
                }
                Some("</td>") => {
                    if inside
                        && let Some(row) = row.as_mut()
                    {
                        // A cell the model closed: the box is the one this
                        // token was given, and the tokens and boxes are one
                        // apiece.
                        row.push(Cell {
                            token: index,
                            across,
                            down,
                        });
                    }
                    inside = false;
                    opened = None;
                    across = 1;
                    down = 1;
                }
                Some(other) => {
                    // Two kinds of span and two counters. Reading either one as
                    // the other is what put a merged label in the wrong place.
                    if let Some(count) = span(other, "colspan") {
                        across = count;
                    } else if let Some(count) = span(other, "rowspan") {
                        down = count;
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
///
/// Which of the two is asked for by name. The model has `colspan="n"` and
/// `rowspan="n"`, both of the form `name="digits"`, and the digits alone do not
/// say which direction a cell grows in.
fn span(name: &str, kind: &str) -> Option<usize> {
    let name = name.trim_start();
    if !name.starts_with(kind) {
        return None;
    }

    name.rsplit('"')
        .nth(1)
        .and_then(|digits| digits.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose long side is its height goes into the square with the
    /// square's stride, not its own.
    ///
    /// The two strides are equal exactly while the picture is wider than it is
    /// tall, which is every table in the corpus — so writing the pixels down in
    /// the order the picture holds them looked correct for as long as anyone
    /// looked. A portrait crop is where they come apart: written with its own
    /// stride, the picture is folded every `SIDE` pixels and arrives sheared,
    /// and the model answers with the right number of rows and boxes that
    /// belong to nothing. Measured on an 1196×1746 bank schedule, which came
    /// back as twenty-eight rows of jumbled text and now comes back as itself.
    #[test]
    fn a_portrait_picture_is_not_folded_into_the_square() {
        // Exactly half, because that makes the resize the identity: a 244×488
        // picture is scaled by 488/488 and lands back at 244×488, so every
        // pixel below is the one that was put there and the arithmetic is the
        // stride and nothing else.
        let (width, height) = (SIDE / 2, SIDE);
        let mut picture = image::RgbImage::from_pixel(width as u32, height as u32, WHITE);
        // The whole second row, black.
        for x in 0..width as u32 {
            picture.put_pixel(x, 1, BLACK);
        }

        let (pixels, size) = squashed(&picture);
        assert_eq!(size, (width as f32, height as f32), "resized when it should not have been");

        // Channel zero on its own: both rows are one colour across, so which
        // channel this is does not matter. Black normalises negative and white
        // positive, so the sign is the row and no constant has to be repeated.
        let at = |x: usize, y: usize| pixels[y * SIDE + x];

        assert!(at(0, 0) > 0.0, "the first row of the picture is not the first row of the square");
        assert!(at(0, 1) < 0.0, "the second row of the picture is not the second row of the square");

        // And where the picture's own stride would have put that black row: one
        // picture-width along the square's first line. That is past the right
        // edge of the picture, so nothing was written there and it is still the
        // zero the square started as.
        assert_eq!(
            at(width, 0),
            0.0,
            "the picture was laid down with its own stride"
        );
    }

    const WHITE: image::Rgb<u8> = image::Rgb([255, 255, 255]);
    const BLACK: image::Rgb<u8> = image::Rgb([0, 0, 0]);

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

        // A table 1470 by 479 resizes to 488 by 159 and is padded to 488.
        let boxes = read_boxes(&located, (488.0, 159.0), 1470.0, 479.0);

        assert_eq!(boxes.len(), 1);

        let (left, top, right, bottom) = boxes[0];
        assert!((left - 0.0).abs() < 1e-3);
        assert!((top - 0.0).abs() < 1e-3);

        // Across is the easy half: the long side was resized *to* the square's
        // side, so half the square across is half the picture across.
        assert!((right - 735.0).abs() < 1.0, "{right}");

        // Down is not, and this is the number two versions of this disagreed
        // about. A quarter of the way down the square is the square's 122nd
        // row, and the picture is only the first 159 of its 488 rows — so it is
        // 0.77 of the way down the *picture*, not a quarter. A wide table is
        // resized to 488 by 159 whatever its own proportions, and a mapping
        // that forgets the second step draws every row of a fifteen-row table
        // as a ten-pixel stripe in the top twelfth of the page.
        assert!((bottom - 367.5).abs() < 1.0, "{bottom}");
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
        assert_eq!(span(" colspan=\"2\"", "colspan"), Some(2));
        assert_eq!(span(" rowspan=\"17\"", "rowspan"), Some(17));
        assert_eq!(span("<tr>", "colspan"), None);
    }

    /// The two spans are two counters, and reading either as the other is what
    /// put a merged label three rows deep into three columns instead.
    #[test]
    fn a_rowspan_is_not_a_colspan() {
        assert_eq!(span(" rowspan=\"3\"", "colspan"), None);
        assert_eq!(span(" colspan=\"3\"", "rowspan"), None);
    }

    /// The sequence the model emitted for a table whose first column is one cell
    /// two rows tall, shortened to the part that matters.
    #[test]
    fn a_cell_that_spans_down_keeps_its_columns() {
        // <tr><td rowspan="2"></td><td></td><td></td></tr>
        let rowspan_two = DICTIONARY
            .iter()
            .position(|entry| *entry == " rowspan=\"2\"")
            .expect("the dictionary has one")
            + 1;

        let tokens = [
            5,
            7,
            rowspan_two as u32,
            9,
            10,
            PLAIN_CELL,
            PLAIN_CELL,
            6,
        ];
        let rows = rows_of(&tokens);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3, "a tall cell is still one cell wide: {rows:?}");
        assert_eq!(rows[0][0].down, 2, "the first cell is two rows tall");
        assert_eq!(rows[0][0].across, 1, "and one column wide");
        assert_eq!(rows[0][1].down, 1);
    }

    /// The sequence the model actually emitted for a register whose first
    /// column is one cell three rows tall — the cell it never closed.
    ///
    /// It writes `<td> rowspan="2" <td >` where it means one tall cell, and the
    /// box that belongs to that cell is the one on the `<td>` that opened it.
    /// Measured: the `<td` that follows carries the box of something else, so
    /// closing the cell onto the later token leaves the merged label with no
    /// cell to land in and it ends up in the header.
    #[test]
    fn a_cell_the_model_never_closed_keeps_the_box_it_opened_on() {
        let rowspan_two = DICTIONARY
            .iter()
            .position(|entry| *entry == " rowspan=\"2\"")
            .expect("the dictionary has one")
            + 1;

        // <tr> <td> rowspan="2" <td > <td></td> <td></td> </tr>
        let tokens = [
            5,
            7,
            rowspan_two as u32,
            8,
            9,
            PLAIN_CELL,
            PLAIN_CELL,
            6,
        ];
        let rows = rows_of(&tokens);

        assert_eq!(rows.len(), 1);
        let first = rows[0][0];
        assert_eq!(first.down, 2, "the merge is a merge: {rows:?}");
        assert_eq!(first.across, 1);
        assert_eq!(first.token, 1, "the box is the one from the token that opened it");
        assert_eq!(rows[0][1].token, 5, "and the words after it are their own cells:
            the cell the model left open is finished at the end of the row, so it
            costs an empty column at the edge rather than putting every word in
            the row one place to the right");
    }

    /// And the other token still widens a row, so the fix for one is not the
    /// loss of the other.
    #[test]
    fn a_colspan_is_still_a_colspan() {
        // <tr><td colspan="2"></td><td></td></tr>
        let colspan_two = DICTIONARY
            .iter()
            .position(|entry| *entry == " colspan=\"2\"")
            .expect("the dictionary has one")
            + 1;

        let tokens = [5, 7, colspan_two as u32, 9, 10, PLAIN_CELL, 6];
        let rows = rows_of(&tokens);

        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[0][0].across, 2);
        assert_eq!(rows[0][0].down, 1);
    }

    /// The picture the model is handed is square, normalised, and in the order
    /// the weights expect.
    #[test]
    fn a_wide_table_is_squashed_into_a_square() {
        let picture = image::RgbImage::from_pixel(1470, 479, image::Rgb([255, 255, 255]));

        let (pixels, shaped) = squashed(&picture);

        assert_eq!(pixels.len(), 3 * SIDE * SIDE);
        assert_eq!(shaped, (488.0, 159.0), "the long side and the short one");
        // White is one before the shift, and the shift is not zero, so a
        // channel is either the top or the bottom of the picture.
        let top = pixels[0];
        let below = pixels[SIDE * (SIDE / 2)];
        assert!(top > 1.5, "{top}");
        assert_eq!(below, 0.0, "past the picture is padding");
    }
}
