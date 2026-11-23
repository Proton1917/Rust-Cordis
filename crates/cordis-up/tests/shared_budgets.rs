use cordis_up::{Limits, decode};

#[test]
fn shared_script_sdk_budget_vectors() {
    for line in include_str!("../../../tests/up1/value-budgets.tsv").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        let limits = Limits {
            max_depth: fields[1].parse().unwrap(),
            max_collection: fields[2].parse().unwrap(),
            max_values: fields[3].parse().unwrap(),
            max_frame: fields[4].parse().unwrap(),
        };
        let bytes = fields[6]
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let result = decode(&bytes, &limits);
        assert_eq!(
            result.is_ok(),
            fields[5] == "1",
            "{}: {result:?}",
            fields[0]
        );
    }
}
