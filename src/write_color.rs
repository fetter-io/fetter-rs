use crossterm::tty::IsTty;
use crossterm::{
    execute,
    style::{Attribute, Color, Print, SetAttribute, SetForegroundColor},
};
use std::io::Write;

fn to_rgb(hex_color: &str) -> (u8, u8, u8) {
    if hex_color.len() == 7 && hex_color.starts_with('#') {
        if let Ok(rgb) = u32::from_str_radix(&hex_color[1..], 16) {
            let r = ((rgb >> 16) & 0xFF) as u8;
            let g = ((rgb >> 8) & 0xFF) as u8;
            let b = (rgb & 0xFF) as u8;
            return (r, g, b);
        }
    }
    panic!("Bad color code: {hex_color}");
}

pub fn write_color<W: Write + IsTty>(writer: &mut W, hex_color: &str, message: &str) {
    if writer.is_tty() {
        let (r, g, b) = to_rgb(hex_color);
        execute!(
            writer,
            SetForegroundColor(Color::Rgb { r, g, b }),
            // SetAttribute(Attribute::Bold),
            Print(message),
            SetAttribute(Attribute::Reset)
        )
        .unwrap();
    } else {
        write!(writer, "{message}").unwrap();
    }
}
