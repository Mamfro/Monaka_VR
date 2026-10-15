use super::*;

#[test]
fn build_trees_keep_runs_in_the_repository() {
    let paths = Paths::for_binaries(Path::new(r"C:\code\monaka\target\release"));
    assert_eq!(paths.runs, Path::new(r"C:\code\monaka\runs"));
    assert_eq!(paths.licences, Path::new(r"C:\code\monaka\licenses"));
    assert_eq!(paths.dev_games.as_deref(), Some(Path::new(r"C:\code\monaka\local\dev-games.txt")));
    let release = Paths::for_binaries(Path::new(r"C:\Games\monaka"));
    assert!(release.runs.ends_with(r"monaka\runs") && release.dev_games.is_none());
    assert_eq!(release.licences, Path::new(r"C:\Games\monaka\licenses"));
}
