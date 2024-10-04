use std::fs::File;
use std::io;
use std::io::{Error, Write};
use std::path::PathBuf;

/// Return a representative row presentation, as strings, for this struct. Note that the number of rows need not be equal to the number of struct fields.
pub(crate) trait Rowable {
    fn to_row(&self) -> Vec<String>;
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

/// Wite a table to a writer. If `delimiter` is None, we assume writing to stdout; if `delimiter` is not None, we assume writing a delimited text file.
fn to_table_writer<W: Write, T: Rowable>(
    writer: &mut W,
    headers: Vec<String>,
    table: &Vec<T>,
    delimiter: Option<&str>,
) -> Result<(), Error> {
    if table.is_empty() || headers.is_empty() {
        return Ok(());
    }
    match delimiter {
        Some(delim) => {
            to_writer_delimited(writer, &headers, delim)?;
            for row in table {
                to_writer_delimited(writer, &row.to_row(), delim)?;
            }
        }
        None => {
            let num_columns = headers.len();
            let mut column_widths = vec![0; num_columns];
            for (i, header) in headers.iter().enumerate() {
                column_widths[i] = header.len();
            }

            let mut rows = Vec::new();
            for row in table {
                let values = row.to_row();
                for (i, value) in values.iter().enumerate() {
                    column_widths[i] = column_widths[i].max(value.len());
                }
                rows.push(values);
            }
            // header
            for (i, header) in headers.into_iter().enumerate() {
                write!(writer, "{:<width$} ", header, width = column_widths[i])?;
            }
            writeln!(writer)?;

            // separator
            for width in &column_widths {
                write!(writer, "{:-<width$} ", "-", width = width)?;
            }
            writeln!(writer)?;

            // body
            for values in rows {
                for (i, value) in values.into_iter().enumerate() {
                    write!(writer, "{:<width$} ", value, width = column_widths[i])?;
                }
                writeln!(writer)?;
            }
        }
    }
    Ok(())
}

pub(crate) trait Tableable<T: Rowable> {
    // fn to_writer<W: Write>(&self, writer: W, delimiter: Option<&str>) -> io::Result<()>;
    fn get_header(&self) -> Vec<String>;

    fn get_records(&self) -> &Vec<T>;

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: Option<&str>,
    ) -> io::Result<()> {
        let _ = to_table_writer(
            &mut writer,
            self.get_header(),
            self.get_records(),
            delimiter,
        );
        Ok(())
    }

    fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file, Some(&delimiter.to_string()))
    }

    fn to_stdout(&self) -> io::Result<()> {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, None)
    }
}
