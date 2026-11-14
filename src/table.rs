use crossterm::terminal;
use crossterm::tty::IsTty;

use std::fs::File;
use std::io;
use std::io::{Error, Write};
use std::path::PathBuf;

// use crate::util::get_writer;
use crate::inspect_report::PY_NAME_KEEP;
use crate::util::get_writer;
use crate::write_color::write_color;

//------------------------------------------------------------------------------
#[derive(PartialEq)]
pub enum RowableContext {
    Delimited,
    Tty,
    // Undefined, // not delimited or tty
}

/// Translate one struct into one or more rows (Vec<String>). Note that the number of resultant columns not be equal to the number of struct fields.
pub trait Rowable {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>>;
}

//------------------------------------------------------------------------------
#[derive(Debug)]
pub(crate) struct WidthFormat {
    width_pad: usize,
    width_chars: usize,
}

fn optimize_widths(
    widths_max: &[usize],
    ellipsisable: &[bool],
    w_gutter: usize,
) -> Vec<WidthFormat> {
    let w_terminal = match terminal::size() {
        Ok((w, _)) => w,
        _ => 0,
    };
    optimize_widths_for(widths_max, ellipsisable, w_gutter, w_terminal)
}

/// Fit columns to `w_terminal` by shrinking only ellipsisable columns, in proportion to their widths. A terminal width of 0 means unknown: no fitting. If the terminal is narrower than the non-ellipsisable columns alone, ellipsisable columns shrink to their minimum and the table overflows rather than failing.
fn optimize_widths_for(
    widths_max: &[usize],
    ellipsisable: &[bool],
    w_gutter: usize,
    w_terminal: u16,
) -> Vec<WidthFormat> {
    // total characters needed; we add a gutter after all columns, even the last one
    let w_total: usize = widths_max.iter().sum::<usize>() + (w_gutter * widths_max.len());
    let ellipsisable_any = ellipsisable.iter().any(|&x| x);

    if !ellipsisable_any || w_total <= w_terminal.into() || w_terminal == 0 {
        return widths_max
            .iter()
            .map(|e| WidthFormat {
                width_chars: *e,
                width_pad: *e + w_gutter,
            })
            .collect();
    }
    let w_excess: f64 = (w_total - w_terminal as usize) as f64; // width to trim
    let mut widths = Vec::new();

    // get the sum of all ellipsisable widths
    let w_ellipsisable: usize = widths_max
        .iter()
        .zip(ellipsisable.iter())
        .filter(|(_, &is_ellipsisable)| is_ellipsisable)
        .map(|(width, _)| width)
        .sum();

    for (i, width) in widths_max.iter().enumerate() {
        if ellipsisable[i] {
            // we derive proportion based on observed max_width
            let proportion = *width as f64 / w_ellipsisable as f64;
            let reduction = (proportion * w_excess) as usize;
            // saturating: the reduction exceeds the column when the terminal is narrower than the non-ellipsisable columns
            let w_field = width.saturating_sub(reduction).max(w_gutter + 1);
            widths.push(WidthFormat {
                width_chars: w_field - w_gutter,
                width_pad: w_field,
            })
        } else {
            widths.push(WidthFormat {
                width_chars: *width,
                width_pad: width + w_gutter,
            });
        }
    }
    widths
}

const ELLIPSIS: char = '…';
const EWIDTH: usize = 1; // ELLIPSIS width

/// Given a string field, trim and ellipse, as well as possible pad field.
fn prepare_field(value: &String, widths: &WidthFormat) -> String {
    if value.len() <= widths.width_chars {
        format!("{:<w$}", value, w = widths.width_pad)
    } else if widths.width_chars > EWIDTH && (value.len() - widths.width_chars) > EWIDTH {
        format!(
            "{:<w$}",
            format!("{}{}", &value[..(widths.width_chars - EWIDTH)], ELLIPSIS),
            w = widths.width_pad
        )
    } else {
        format!("{:<w$}", &value[..widths.width_chars], w = widths.width_pad)
    }
}

fn to_table_delimited<W: Write, T: Rowable>(
    writer: &mut W,
    column_formats: Vec<ColumnFormat>,
    records: &Vec<T>,
    delimiter: &str,
) -> Result<(), Error> {
    if records.is_empty() || column_formats.is_empty() {
        return Ok(());
    }
    let header_labels: Vec<String> =
        column_formats.iter().map(|hf| hf.header.clone()).collect();
    writeln!(writer, "{}", header_labels.join(delimiter))?;
    for record in records {
        for row in record.to_rows(&RowableContext::Delimited) {
            writeln!(writer, "{}", row.join(delimiter))?;
        }
    }
    Ok(())
}

/// Write Rowables to a writer. If `delimiter` is None, we assume writing to stdout; if `delimiter` is not None, we assume writing a delimited text file.
fn to_table_display<W: Write + IsTty, T: Rowable>(
    writer: &mut W,
    column_formats: Vec<ColumnFormat>,
    records: &Vec<T>,
) -> Result<(), Error> {
    if records.is_empty() || column_formats.is_empty() {
        return Ok(());
    }
    let header_labels: Vec<String> =
        column_formats.iter().map(|hf| hf.header.clone()).collect();
    let ellipsisable: Vec<bool> =
        column_formats.iter().map(|hf| hf.ellipsisable).collect();

    // evaluate column_formats and all elements in every row to determine max column widths; store extracted rows for reuse in writing body.
    let mut widths_max = vec![0; column_formats.len()];
    for (i, header) in header_labels.iter().enumerate() {
        widths_max[i] = header.len();
    }
    // Collect results of calling to_rows() on each record into a new Vec
    let mut rows = Vec::new();
    for record in records {
        for row in record.to_rows(&RowableContext::Tty) {
            for (i, element) in row.iter().enumerate() {
                widths_max[i] = widths_max[i].max(element.len());
            }
            rows.push(row);
        }
    }

    // if a ColumnFormat sets a width_limit, would need to force some columns to be ellipsisable that may not already be; and would need to send in a different vector of width_target
    let w_gutter = 2;
    let widths = optimize_widths(&widths_max, &ellipsisable, w_gutter);
    // header
    for (i, header) in header_labels.into_iter().enumerate() {
        write_color(
            writer,
            &column_formats[i].color,
            &prepare_field(&header, &widths[i]),
        );
        // write!(writer, "{}", prepare_field(&header, &widths[i]),)?;
    }
    writeln!(writer)?;
    // body
    for row in rows {
        for (i, element) in row.into_iter().enumerate() {
            let _ = column_formats[i].write_element(writer, &element, &widths[i]);
        }
        writeln!(writer)?;
    }
    Ok(())
}

//------------------------------------------------------------------------------
pub struct ColumnFormat {
    header: String,
    ellipsisable: bool,
    color: String,
}

impl ColumnFormat {
    pub(crate) fn new(header: String, ellipsisable: bool, color: String) -> ColumnFormat {
        ColumnFormat {
            header,
            ellipsisable,
            color,
        }
    }

    // All terminal content uses this method to write output. This permits per-column formatting based on the column type, discovered via the header string.
    pub(crate) fn write_element<W: Write + IsTty>(
        &self,
        writer: &mut W,
        message: &String,
        width_format: &WidthFormat,
    ) -> Result<(), Error> {
        let field = prepare_field(message, width_format);
        if self.header == "Package" {
            // split on hyphen
            let parts: Vec<&str> = field.split('-').collect();
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    write_color(writer, "#ff9900", "-");
                }
                write!(writer, "{part}")?;
            }
        } else if self.header == "Site" {
            write_color(writer, "#999999", &field);
        } else if self.header == "File" {
            // used in InspectReport
            if PY_NAME_KEEP.contains(&message.as_str()) {
                write_color(writer, "#FF999C", &field);
            } else {
                write!(writer, "{field}")?;
            }
        } else {
            write!(writer, "{field}")?;
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
pub trait Tableable<T: Rowable> {
    fn get_header(&self) -> Vec<ColumnFormat>;
    fn get_records(&self) -> &Vec<T>;

    fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let mut file = File::create(file_path)?;
        to_table_delimited(
            &mut file,
            self.get_header(),
            self.get_records(),
            &delimiter.to_string(),
        )
    }

    fn to_writer(&self, stderr: bool) -> io::Result<()> {
        let mut writer = get_writer(stderr);
        to_table_display(&mut writer, self.get_header(), self.get_records())
    }
}

#[cfg(test)]
mod tests_widths {
    use super::*;

    #[test]
    fn test_optimize_widths_fits_a() {
        // fits: unchanged
        let w = optimize_widths_for(&[10, 40], &[false, true], 2, 100);
        assert_eq!((w[1].width_chars, w[1].width_pad), (40, 42));
        // unknown terminal width: unchanged
        let w = optimize_widths_for(&[10, 40], &[false, true], 2, 0);
        assert_eq!(w[1].width_chars, 40);
    }

    #[test]
    fn test_optimize_widths_shrinks_a() {
        // 10+2 + 40+2 = 54 needed, 34 available: only the ellipsisable column shrinks, and the row fits
        let w = optimize_widths_for(&[10, 40], &[false, true], 2, 34);
        assert_eq!((w[0].width_chars, w[0].width_pad), (10, 12));
        assert!(w[1].width_chars < 40);
        assert!(w[0].width_pad + w[1].width_pad <= 34);
    }

    #[test]
    fn test_optimize_widths_narrow_terminal_a() {
        // the non-ellipsisable column alone exceeds the terminal: no underflow, minimum widths
        let w = optimize_widths_for(&[60, 40, 30], &[false, true, true], 2, 20);
        assert_eq!(w[0].width_chars, 60);
        assert!(w[1].width_chars >= 1 && w[1].width_pad == w[1].width_chars + 2);
        assert!(w[2].width_chars >= 1 && w[2].width_pad == w[2].width_chars + 2);
        // fields render without panicking
        let _ = prepare_field(&"x".repeat(100), &w[1]);
        let _ = prepare_field(&"y".to_string(), &w[2]);
    }
}
