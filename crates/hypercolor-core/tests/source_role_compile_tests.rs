#[test]
fn source_roles_reject_invalid_registration() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/source_roles/*.rs");
}
