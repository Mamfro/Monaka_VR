use super::*;

fn digest(data: &[u8]) -> String {
    let mut hash = Sha256::default();
    hash.update(data);
    hex(&hash.finish())
}

#[test]
fn known_vectors() {
    assert_eq!(digest(b""), "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855");
    assert_eq!(digest(b"abc"), "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    assert_eq!(
        digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248D6A61D20638B8E5C026930C3E6039A33CE45964FF2167F6ECEDD419DB06C1"
    );
}

#[test]
fn split_updates_match_one_update() {
    let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
    let mut split = Sha256::default();
    for chunk in data.chunks(37) {
        split.update(chunk);
    }
    assert_eq!(hex(&split.finish()), digest(&data));
}
