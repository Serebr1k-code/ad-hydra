//! ForcAD response marker decoding for the built-in submitter.

pub(super) fn parse_results(stdout: &[u8], expected: usize) -> Option<Vec<String>> {
    let text = std::str::from_utf8(stdout).ok()?;
    let line = text
        .lines()
        .find(|line| line.starts_with("HYDRA_FORCAD_RESULTS="))?;
    let values: Vec<String> =
        serde_json::from_str(line.strip_prefix("HYDRA_FORCAD_RESULTS=")?).ok()?;
    (values.len() == expected
        && values.iter().all(|value| {
            matches!(
                value.as_str(),
                "accepted"
                    | "already_stolen"
                    | "own"
                    | "invalid"
                    | "credential"
                    | "retry"
                    | "unknown"
            )
        }))
    .then_some(values)
}
