use praxis_plugin_api::web::WebDescriptor;

#[test]
fn web_contributions_cannot_claim_host_routes_or_other_packages() {
    let valid = WebDescriptor::for_package("vm", "Virtual machines");
    valid.validate("vm").unwrap();
    for path in [
        "/api/contexts",
        "/plugins/other/ui/page.html",
        "https://evil.test/ui",
        "/plugins/vm/../contexts",
        "/plugins/vm/ui/page.html?token=secret",
    ] {
        let mut invalid = valid.clone();
        invalid.page = path.into();
        assert!(invalid.validate("vm").is_err(), "accepted {path}");
    }
    for id in ["", "..", "a/b", "vm?x", "<script>"] {
        assert!(WebDescriptor::for_package(id, "Invalid")
            .validate(id)
            .is_err());
    }
    assert!(valid.validate("other").is_err());
}
