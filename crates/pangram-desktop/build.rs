use cxx_qt_build::{CxxQtBuilder, QmlModule};

const QML_FILES: &[&str] = &[
    "qml/Main.qml",
    "qml/HistoryPane.qml",
    "qml/DocumentPane.qml",
    "qml/ResultsPane.qml",
    "qml/SettingsDialog.qml",
    "qml/FindBar.qml",
    "qml/NoticeBar.qml",
    "qml/ButtonRow.qml",
    "qml/RawDialog.qml",
    "qml/UsageWindow.qml",
];

/// Asks qmake for Qt's QML install directory, used at runtime to detect optional styles.
fn qt_install_qml() -> String {
    let qmake = std::env::var("QMAKE").unwrap_or_else(|_| "qmake6".to_owned());
    std::process::Command::new(&qmake)
        .args(["-query", "QT_INSTALL_QML"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "/usr/lib64/qt6/qml".to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=QMAKE");
    println!(
        "cargo:rustc-env=PANGRAM_QT_INSTALL_QML={}",
        qt_install_qml()
    );
    for f in QML_FILES {
        println!("cargo:rerun-if-changed={f}");
    }

    let builder =
        CxxQtBuilder::new_qml_module(QmlModule::new("net.batkin.pangram").qml_files(QML_FILES))
            .qt_module("Quick")
            .qt_module("QuickControls2")
            .qt_module("Widgets")
            .files(["src/backend.rs"]);
    // SAFETY: only adds a warning flag; Qt 6.11 headers trip GCC 16's -Wsfinae-incomplete.
    let builder = unsafe {
        builder.cc_builder(|cc| {
            cc.flag_if_supported("-Wno-sfinae-incomplete");
        })
    };
    builder.build();
}
