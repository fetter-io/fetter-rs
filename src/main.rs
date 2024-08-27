

mod util;




fn main() {
    let x = util::gcd(3, 20).unwrap();
    println!("{}", x.to_string());
}
