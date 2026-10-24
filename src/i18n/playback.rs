pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "Station" => "Stazione",
        "Mix" => "Mix",
        "Flow" => "Flow",
        "Up next" => "In riproduzione successiva",
        "Loading more tracks..." => "Caricamento di altri brani...",
        "Loading more tracks" => "Caricamento di altri brani",
        "Nothing up next" => "Nessun brano in coda",
        "Extending this personalized queue." => "La coda personalizzata è in aggiornamento.",
        "You've reached the end of the queue." => "Hai raggiunto la fine della coda.",
        "Pop out" => "Sposta in una finestra",
        "Disable always on top" => "Disattiva sempre in primo piano",
        "Enable always on top" => "Attiva sempre in primo piano",
        "Shuffle" => "Casuale",
        "Previous" => "Precedente",
        "Next" => "Successivo",
        "Play" => "Riproduci",
        "Pause" => "Pausa",
        "Repeat off" => "Ripetizione disattivata",
        "Repeat all" => "Ripeti tutto",
        "Repeat one" => "Ripeti un brano",
        "playback disabled, blocked content" => "riproduzione disattivata, contenuto bloccato",
        "Mute / Unmute" => "Disattiva / Riattiva audio",
        "Close Player" => "Chiudi player",
        "Download" => "Scarica",
        "Lyrics" => "Testo",
        "Queue" => "Coda",
        "Favorite Track" => "Brano preferito",
        "Loading audio..." => "Caricamento audio...",
        "Playback failed" => "Riproduzione non riuscita",
        "Loading..." => "Caricamento...",
        "Nothing playing" => "Nessuna riproduzione",
        "Added to end of queue" => "Aggiunto alla fine della coda",
        "Playing next" => "In riproduzione successiva",
        "Playback error" => "Errore di riproduzione",
        "Could not seek" => "Impossibile spostarsi nel brano",
        "AI content blocking unavailable" => {
            "Blocco dei contenuti generati dall'IA non disponibile"
        }
        "Sign in to Deezer so ralgruM can identify AI generated tracks." => {
            "Accedi a Deezer per permettere a ralgruM di riconoscere i brani generati dall'IA."
        }
        "The AI metadata client could not be created." => {
            "Impossibile creare il client dei metadati IA."
        }
        "Could not restore playback position" => {
            "Impossibile ripristinare la posizione di riproduzione"
        }
        "The audio output could not be restored" => "Impossibile ripristinare l'uscita audio",
        "Could not switch output" => "Impossibile cambiare uscita audio",
        "The audio output stopped responding" => "L'uscita audio ha smesso di rispondere",
        "Playback was paused. Pick a working output to continue." => {
            "La riproduzione è stata messa in pausa. Scegli un'uscita funzionante per continuare."
        }
        "Could not play \"{}\": {}" => "Impossibile riprodurre \"{}\": {}",
        "1 track" => "1 brano",
        "{} track" => "{} brano",
        "{} tracks" => "{} brani",
        "track" => "brano",
        "tracks" => "brani",
        "track added" => "brano aggiunto",
        "tracks added" => "brani aggiunti",
        _ => return None,
    })
}
