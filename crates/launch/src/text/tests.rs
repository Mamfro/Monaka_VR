use super::*;

#[test]
fn reads_a_steam_install_folder() {
    let acf = "\"AppState\"
{
	\"appid\"		\"379430\"
	\"installdir\"		\"KingdomComeDeliverance\"
}";
    assert_eq!(acf_install_dir(acf).as_deref(), Some("KingdomComeDeliverance"));
    assert_eq!(acf_install_dir("\"appid\" \"1\""), None);
}

#[test]
fn reads_the_steamvr_runtime() {
    let json = r#"{ "config" : [ "C:\\Program Files (x86)\\Steam\\config" ], "runtime" : [ "C:\\Program Files (x86)\\Steam\\steamapps\\common\\SteamVR", "D:\\Other" ], "version" : 1 }"#;
    assert_eq!(vrpath_runtime(json).as_deref(), Some(r"C:\Program Files (x86)\Steam\steamapps\common\SteamVR"));
    assert_eq!(vrpath_runtime(r#"{"runtime":["C:\/a\u0041"]}"#).as_deref(), Some("C:/aA"));
    assert_eq!(vrpath_runtime(r#"{"runtime":[]}"#), None);
    assert_eq!(vrpath_runtime("{}"), None);
}

#[test]
fn reads_steam_libraries() {
    let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
    assert_eq!(vdf_library_paths(vdf), [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
}

#[test]
fn reads_sizes() {
    assert_eq!(eye_image_size("Runtime: SteamVR\nEye image size: 2880 2880\n"), Some((2880, 2880)));
    assert_eq!(eye_image_size("Eye image size: 12"), None);
}
