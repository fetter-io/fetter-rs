use crossterm::terminal;
use crossterm::tty::IsTty;
// use crossterm::{
//     execute,
//     style::{Attribute, Color, Print, SetAttribute, SetForegroundColor},
// };
use std::fs::File;
use std::io;
use std::io::{Error, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

// use crate::util::get_writer;
use crate::util::get_writer;
use crate::write_color::write_color;

//------------------------------------------------------------------------------
#[derive(PartialEq)]
pub(crate) enum RowableContext {
    Delimited,
    Tty,
    // Undefined, // not delimited or tty
}

/// Translate one struct into one or more rows (Vec<String>). Note that the number of resultant columns not be equal to the number of struct fields.
pub(crate) trait Rowable {
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
    // total characters needed; we add a gutter after all columns, even the last one
    let w_total: usize = widths_max.iter().sum::<usize>() + (w_gutter * widths_max.len());
    let ellipsisable_any = ellipsisable.iter().any(|&x| x);
    let w_terminal = match terminal::size() {
        Ok((w, _)) => w,
        _ => 0,
    };

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

    let w_ellipsisable: usize = widths_max
        .iter()
        .zip(ellipsisable.iter())
        .filter(|(_, &is_ellipsisable)| is_ellipsisable)
        .map(|(width, _)| width)
        .sum();

    for (i, width) in widths_max.iter().enumerate() {
        if ellipsisable[i] {
            let proportion = *width as f64 / w_ellipsisable as f64;
            let reduction = (proportion * w_excess) as usize;
            let w_field = (*width - reduction).max(3);
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

fn prepare_field(value: &String, widths: &WidthFormat) -> String {
    if value.len() <= widths.width_chars {
        format!("{:<w$}", value, w = widths.width_pad)
    } else if widths.width_chars > 3 && (value.len() - widths.width_chars) > 3 {
        format!(
            "{:<w$}",
            format!("{}...", &value[..(widths.width_chars - 3)]),
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

/// Wite Rowables to a writer. If `delimiter` is None, we assume writing to stdout; if `delimiter` is not None, we assume writing a delimited text file.
fn to_table_display<W: Write + AsRawFd, T: Rowable>(
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
    // evaluate column_formats and all elements in every row to determine max colum widths; store extracted rows for reuse in writing body.
    let mut widths_max = vec![0; column_formats.len()];
    for (i, header) in header_labels.iter().enumerate() {
        widths_max[i] = header.len();
    }
    let mut rows = Vec::new();
    for record in records {
        for row in record.to_rows(&RowableContext::Tty) {
            for (i, element) in row.iter().enumerate() {
                widths_max[i] = widths_max[i].max(element.len());
            }
            rows.push(row);
        }
    }
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
pub(crate) struct ColumnFormat {
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
                write!(writer, "{}", part)?;
            }
        } else if self.header == "Site" {
            write_color(writer, "#999999", &field);
        // } else if message.starts_with("#") {
        //     write_color(writer, "#999999", &field);
        } else {
            write!(writer, "{}", field)?;
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
pub(crate) trait Tableable<T: Rowable> {
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
