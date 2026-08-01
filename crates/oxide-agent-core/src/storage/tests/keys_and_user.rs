use super::*;

#[test]
fn generate_flow_id_returns_v4_uuid() {
    let flow_id = generate_flow_id();
    let parsed = Uuid::parse_str(&flow_id);
    assert!(parsed.is_ok());
    let version = parsed.map(|uuid| uuid.get_version_num());
    assert_eq!(version, Ok(4));
}
