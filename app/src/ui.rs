//! Minimal egui/eframe desktop window. Compiled only under the `gui` feature so the
//! default (headless) smoke build needs no display/windowing system libraries.

use eframe::egui;

pub fn run() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Ten-Cent Poker",
        native_options,
        Box::new(|_cc| Ok(Box::new(PokerApp::default()) as Box<dyn eframe::App>)),
    )
}

#[derive(Default)]
struct PokerApp {
    address: String,
    table_uri: String,
}

impl eframe::App for PokerApp {
    // eframe 0.34: the framework hands us the central `Ui` directly.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Ten-Cent Poker");
        ui.separator();

        if ui.button("Load dev wallet (account 0)").clicked() {
            if let Ok(addr) = poker_wallet::derive_address(poker_wallet::DEV_MNEMONIC, 0) {
                self.address = addr.to_string();
            }
        }
        ui.label(format!("Wallet: {}", self.address));

        if ui.button("Create table URI").clicked() {
            self.table_uri = poker_net::sample_table_uri();
        }
        ui.label(format!("Table:  {}", self.table_uri));
    }
}
