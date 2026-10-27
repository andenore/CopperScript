//! `math.hypot` and `sum` against values recorded from Python.

use copper_physical::pyfloat;

#[test]
fn hypot_and_sum_match_python() {
    let fixture = include_str!("fixtures/python_floats.txt");
    let float = |text: &str| f64::from_bits(text.parse().expect("bits"));
    let (mut hypots, mut sums) = (0, 0);
    for line in fixture.lines().filter(|line| !line.starts_with('#')) {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("hypot") => {
                let values: Vec<f64> = words.map(float).collect();
                let result = pyfloat::hypot(values[0], values[1]);
                assert_eq!(result.to_bits(), values[2].to_bits(), "hypot({}, {})", values[0], values[1]);
                hypots += 1;
            }
            Some("sum") => {
                let rest: Vec<&str> = words.collect();
                let (items, expected) = rest.split_at(rest.len() - 2);
                let result = pyfloat::sum(items.iter().map(|item| float(item)));
                assert_eq!(result.to_bits(), float(expected[1]).to_bits(), "sum({items:?})");
                sums += 1;
            }
            _ => panic!("unexpected fixture line {line}"),
        }
    }
    assert_eq!((hypots, sums), (406, 300));
}
