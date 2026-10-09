//! The few text formats the launcher reads: SteamVR's `openvrpaths.vrpath` (JSON), Steam's
//! `libraryfolders.vdf` and the viewer's `--info` output. Only the fields needed are read; no
//! general parser.

/// The first SteamVR runtime folder in `openvrpaths.vrpath` (`"runtime": ["..."]`).
pub fn vrpath_runtime(json: &str) -> Option<String> {
    let after = &json[json.find("\"runtime\"")? + "\"runtime\"".len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('[')?.trim_start();
    json_string(after)
}

/// A JSON string literal at the start of `text`, unescaped.
fn json_string(text: &str) -> Option<String> {
    let mut chars = text.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let code: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                }
                other => out.push(other), // \\ \/ \"
            },
            c => out.push(c),
        }
    }
}

/// Every library folder in Steam's `libraryfolders.vdf` (its `"path"` values).
pub fn vdf_library_paths(vdf: &str) -> Vec<String> {
    vdf.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("\"path\"")?.trim_start();
            let value = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(value.replace("\\\\", "\\"))
        })
        .collect()
}

/// The install folder name in a Steam app manifest (`appmanifest_N.acf`: `"installdir" "Prey"`).
pub fn acf_install_dir(acf: &str) -> Option<String> {
    acf.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("\"installdir\"")?.trim_start();
        Some(rest.strip_prefix('"')?.strip_suffix('"')?.to_owned())
    })
}

/// The eye image size the viewer prints with `--info` (`Eye image size: W H`).
pub fn eye_image_size(output: &str) -> Option<(u32, u32)> {
    let line = output.lines().find_map(|l| l.trim().strip_prefix("Eye image size:"))?;
    let mut parts = line.split_whitespace().map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??))
}

#[cfg(test)]
mod tests {
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
}
