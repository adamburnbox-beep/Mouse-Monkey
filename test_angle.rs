fn main() {
    let cx = 1064.0;
    let cy = 500.0;
    let mouse_x = 1000.0;
    let mouse_y = 500.0;
    let angle = (mouse_y - cy).atan2(mouse_x - cx);
    let sector_size = 2.0 * std::f32::consts::PI / 8.0;
    let normalized_angle = (angle + 2.0 * std::f32::consts::PI) % (2.0 * std::f32::consts::PI);
    let sector_index = ((normalized_angle + std::f32::consts::PI / 8.0) / sector_size).floor() as u32 % 8;
    println!("angle: {}, norm: {}, sector: {}", angle, normalized_angle, sector_index);
}
