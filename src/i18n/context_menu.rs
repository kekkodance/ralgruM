pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "Add to playlist" => "Aggiungi alla playlist",
        "Local" => "Locale",
        "Import to Local" => "Importa in Locale",
        "Play" => "Riproduci",
        "Play now" => "Riproduci ora",
        "Play next in queue" => "Riproduci dopo",
        "Play last in queue" => "Riproduci per ultimo",
        "Play mix" => "Riproduci mix",
        "Play Flow" => "Riproduci Flow",
        "Play selection" => "Riproduci selezione",
        "Open" => "Apri",
        "Move up" => "Sposta su",
        "Move down" => "Sposta giù",
        "Remove from queue" => "Rimuovi dalla coda",
        "Remove from cache" => "Rimuovi dalla cache",
        "Remove from Local" => "Rimuovi da Locale",
        "Save to Local" => "Salva in Locale",
        "Favorite" => "Aggiungi ai preferiti",
        "Unfavorite" => "Rimuovi dai preferiti",
        "Loading..." => "Caricamento...",
        "Info" => "Informazioni",
        "Edit" => "Modifica",
        "Delete playlist" => "Elimina playlist",
        "Track" => "Brano",
        "Lyrics" => "Testo",
        "Station" => "Stazione",
        "Similar artists" => "Artisti simili",
        "Mix" => "Mix",
        "Not interested in" => "Non mi interessa",
        "Not interested in this artist" => "Questo artista non mi interessa",
        "Not interested in this track" => "Questo brano non mi interessa",
        "Copy name" => "Copia nome",
        "Copy title" => "Copia titolo",
        "Copy link" => "Copia link",
        "Copy line" => "Copia riga",
        "Copy text block" => "Copia blocco di testo",
        "Copy entire lyrics" => "Copia tutto il testo",
        "Title" => "Titolo",
        "Artist name" => "Nome artista",
        "Album name" => "Nome album",
        "Playlist name" => "Nome playlist",
        "Download" => "Scarica",
        "Download format" => "Formato di download",
        "Best available" => "Migliore disponibile",
        "Best available quality" => "Migliore qualità disponibile",
        "Original file" => "File originale",
        "Original quality" => "Qualità originale",
        "High quality" => "Alta qualità",
        "Lossless / high quality" => "Lossless / alta qualità",
        "MP3 128 kbps" => "MP3 128 kbps",
        "MP3 320 kbps" => "MP3 320 kbps",
        "Lossless quality" => "Qualità lossless",
        "Standard quality" => "Qualità standard",
        "Lossless" => "Lossless",
        "Retry format check" => "Riprova a verificare i formati",
        "Checking available formats..." => "Verifica dei formati disponibili...",
        "No downloadable formats available" => "Nessun formato disponibile per il download",
        "Format check failed" => "Verifica dei formati non riuscita",
        "Checking size and bitrate…" => "Verifica di dimensioni e bitrate…",
        "Playing source" => "Sorgente di riproduzione",
        "Size unavailable · Bitrate unavailable" => {
            "Dimensioni non disponibili · Bitrate non disponibile"
        }
        "Audio information" => "Informazioni audio",
        "Download started" => "Download avviato",
        "Downloads started" => "Download avviati",
        "Download failed" => "Download non riuscito",
        "Downloads failed" => "Download non riusciti",
        "Downloads finished with errors" => "Download completati con errori",
        "Quality unavailable" => "Qualità non disponibile",
        "Queued {count} tracks." => "Brani aggiunti alla coda: {count}.",
        "{saved} saved; {failed} unavailable in that format." => {
            "Salvati: {saved}; non disponibili in questo formato: {failed}."
        }
        "Cut" => "Taglia",
        "Copy" => "Copia",
        "Paste" => "Incolla",
        "Select all" => "Seleziona tutto",
        "Copied to the clipboard." => "Copiato negli appunti.",
        "Link copied" => "Link copiato",
        "Title copied" => "Titolo copiato",
        "Artist name copied" => "Nome artista copiato",
        "Album name copied" => "Nome album copiato",
        "Playlist name copied" => "Nome playlist copiato",
        "Lyric line copied" => "Riga del testo copiata",
        "Lyrics block copied" => "Blocco del testo copiato",
        "Lyrics copied" => "Testo copiato",
        "The downloads folder could not be opened" => "Impossibile aprire la cartella dei download",
        "The completed file could not be revealed" => "Impossibile mostrare il file completato",
        "The downloads folder could not be created" => {
            "Impossibile creare la cartella dei download"
        }
        "The downloads folder could not be created: {error}" => {
            "Impossibile creare la cartella dei download: {error}"
        }
        "The downloads directory could not be read from settings" => {
            "Impossibile leggere la cartella dei download dalle impostazioni"
        }
        "Download capability probe stopped unexpectedly" => {
            "La verifica delle opzioni di download si è interrotta inaspettatamente"
        }
        "The download destination check stopped unexpectedly" => {
            "La verifica della cartella di destinazione si è interrotta inaspettatamente"
        }
        "The batch destination check stopped unexpectedly" => {
            "La verifica delle cartelle di destinazione si è interrotta inaspettatamente"
        }
        "The download worker stopped unexpectedly" => {
            "Il processo di download si è interrotto inaspettatamente"
        }
        "The partial download could not be created" => {
            "Impossibile creare il file temporaneo del download"
        }
        "The partial download could not be reset" => {
            "Impossibile reimpostare il file temporaneo del download"
        }
        "The audio cache is unavailable" => "La cache audio non è disponibile",
        "The completed download could not be moved into place: {error}" => {
            "Impossibile spostare il download completato nella cartella di destinazione: {error}"
        }
        "Download request cancelled" => "Download annullato",
        _ => return None,
    })
}
