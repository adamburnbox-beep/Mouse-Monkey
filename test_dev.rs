use std::os::unix::fs::FileTypeExt;
fn main() {
    let md = std::fs::metadata("/dev/input/event0").unwrap();
    println!("is_file: {}", md.is_file());
    println!("is_char_device: {}", md.file_type().is_char_device());
}
