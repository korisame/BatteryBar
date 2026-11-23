# BatteryBar

Utility per la barra dei menu di macOS. Mostra lo stato della batteria, il tempo stimato alla carica o alla scarica e il consumo energetico. La cronologia viene conservata sul Mac.

## Installazione

Scarica lo ZIP per Apple Silicon dalla [pagina delle release](https://github.com/korisame/BatteryBar/releases), estrai BatteryBar.app e spostala in Applicazioni. La build 10 richiede macOS 26 o successivo. Il pacchetto ha firma ad hoc e macOS può richiedere una conferma all'apertura.

## Sviluppo

Servono Rust, gli strumenti da riga di comando di Xcode e un Mac. Dalla cartella `rust`, esegui `cargo fetch --locked` e `cargo test --release --locked`. Dalla radice, `./build.sh` produce un bundle di sviluppo in `rust/dist`. Per avviarlo con dati separati:

```sh
rust/dist/BatteryBar-candidate.noindex/BatteryBar.app/Contents/MacOS/BatteryBar --isolated --config-dir "$(mktemp -d)"
```
