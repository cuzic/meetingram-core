// Prints the char count and a simple checksum of argv[1] (or of stdin with "-").
use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = if a.get(1).map(|x| x == "-").unwrap_or(false) {
        let mut b = String::new();
        std::io::stdin().read_to_string(&mut b).unwrap();
        b
    } else {
        a.get(1).cloned().unwrap_or_default()
    };
    let sum: u64 = s.chars().fold(0u64, |h, c| h.wrapping_mul(31).wrapping_add(c as u64));
    println!("{} {}", s.chars().count(), sum);
}
