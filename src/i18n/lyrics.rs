pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "No Musixmatch lyrics found" => "Nessun testo trovato su Musixmatch",
        "Try switching to Genius above." => "Prova a passare a Genius qui sopra.",
        "No Genius lyrics found" => "Nessun testo trovato su Genius",
        "Try switching to Musixmatch above." => "Prova a passare a Musixmatch qui sopra.",
        "No Genius Lyrics Found" => "Nessun testo trovato su Genius",
        "The matching Genius song entry does not include plain lyrics." => {
            "La voce del brano su Genius non include il testo normale."
        }
        "No Genius lyrics were found for this track." => {
            "Non è stato trovato il testo di questo brano su Genius."
        }
        "Finding lyrics" => "Ricerca dei testi",
        "Searching for lyrics for this track..." => "Ricerca del testo di questo brano...",
        "No lyrics found" => "Nessun testo trovato",
        "Neither provider has lyrics for this track." => {
            "Nessuno dei due servizi ha il testo di questo brano."
        }
        "Lyrics failed to load" => "Impossibile caricare i testi",
        "No track playing" => "Nessun brano in riproduzione",
        "Play a track to view lyrics." => "Riproduci un brano per vedere i testi.",
        "Lyrics" => "Testi",
        "Disable always on top" => "Disattiva sempre in primo piano",
        "Enable always on top" => "Attiva sempre in primo piano",
        "Pop out" => "Apri in una finestra separata",
        _ => return None,
    })
}
