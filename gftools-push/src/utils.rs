use std::path::{Path, PathBuf};

pub(crate) fn repo_path_to_google_path(fp: &Path) -> PathBuf {
    // We rename lang paths due to: https://github.com/google/fonts/pull/4679
    let parts = fp.components().map(|c| c.as_os_str()).collect::<Vec<_>>();
    if parts.iter().any(|c| *c == "gflanguages") {
        return Path::new("lang").join(fp.strip_prefix("lang/Lib/gflanguages/data").unwrap());
    }
    // https://github.com/google/fonts/pull/5147
    else if parts.iter().any(|c| *c == "axisregistry") {
        return Path::new("axisregistry").join(fp.file_name().unwrap());
    }
    fp.to_path_buf()
}

pub(crate) fn google_path_to_repo_path(fp: &Path) -> PathBuf {
    let parts = fp.components().map(|c| c.as_os_str()).collect::<Vec<_>>();
    if parts.iter().any(|c| *c == "lang") && !parts.iter().any(|c| *c == "site-packages") {
        return Path::new("lang/Lib/gflanguages/data/").join(fp.strip_prefix("lang").unwrap());
    } else if parts.iter().any(|c| *c == "axisregistry")
        && !parts.iter().any(|c| *c == "site-packages")
    {
        return fp
            .parent()
            .unwrap()
            .join("Lib/axisregistry/data")
            .join(fp.file_name().unwrap());
    }
    fp.to_path_buf()
}
