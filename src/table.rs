use std::fs::File;
use std::io;
use std::io::{Error, Write};
use std::path::PathBuf;

#[derive(PartialEq)]
pub(crate) enum RowableContext {
    Delimited,
    TTY,
    // Undefined, // not delimited or tty
}

/// Translate one struct into one or more rows (Vec<String>). Note that the number of resultant columns not be equal to the number of struct fields.
pub(crate) trait Rowable {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>>;
}

fn to_writer_delimited<W: Write>(
    writer: &mut W,
    row: &[String],
    delimiter: &str,
) -> Result<(), Error> {
    let row_str = row.join(delimiter);
    writeln!(writer, "{}", row_str)?;
    Ok(())
}

/// Wite Rowables to a writer. If `delimiter` is None, we assume writing to stdout; if `delimiter` is not None, we assume writing a delimited text file.
fn to_table_writer<W: Write, T: Rowable>(
    writer: &mut W,
    headers: Vec<String>,
    records: &Vec<T>,
    delimiter: Option<&str>,
    context: RowableContext,
) -> Result<(), Error> {
    if records.is_empty() || headers.is_empty() {
        return Ok(());
    }
    match delimiter {
        Some(delim) => {
            to_writer_delimited(writer, &headers, delim)?;
            for record in records {
                for row in record.to_rows(&context) {
                    to_writer_delimited(writer, &row, delim)?;
                }
            }
        }
        None => {
            // evaluate headers and all elements in every row to determine max colum widths; store extracted rows for reuse in writing body.
            let mut column_widths = vec![0; headers.len()];
            for (i, header) in headers.iter().enumerate() {
                column_widths[i] = header.len();
            }
            let mut rows = Vec::new();
            for record in records {
                for row in record.to_rows(&context) {
                    for (i, element) in row.iter().enumerate() {
                        column_widths[i] = column_widths[i].max(element.len());
                    }
                    rows.push(row);
                }
            }
            // header
            for (i, header) in headers.into_iter().enumerate() {
                write!(writer, "{:<width$} ", header, width = column_widths[i])?;
            }
            writeln!(writer)?;
            // separator
            // for width in &column_widths {
            //     write!(writer, "{:_<width$} ", "_", width = width)?;
            // }
            // writeln!(writer)?;
            // body
            for row in rows {
                for (i, element) in row.into_iter().enumerate() {
                    write!(writer, "{:<width$} ", element, width = column_widths[i])?;
                }
                writeln!(writer)?;
            }
        }
    }
    Ok(())
}

pub(crate) trait Tableable<T: Rowable> {
    fn get_header(&self) -> Vec<String>;
    fn get_records(&self) -> &Vec<T>;

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: Option<&str>,
        context: RowableContext,
    ) -> io::Result<()> {
        let _ = to_table_writer(
            &mut writer,
            self.get_header(),
            self.get_records(),
            delimiter,
            context,
        );
        Ok(())
    }

    fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(
            file,
            Some(&delimiter.to_string()),
            RowableContext::Delimited,
        )
    }

    fn to_stdout(&self) -> io::Result<()> {
        let stdout = io::stdout();
        let handle = stdout.lock();
        // TODO: check if we are a TTY
        self.to_writer(handle, None, RowableContext::TTY)
    }
}
