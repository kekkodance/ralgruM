pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "Search songs, artists, or albums..." => "Cerca brani, artisti o album...",
        "Search suggestions" => "Suggerimenti di ricerca",
        "Remove from recent searches" => "Rimuovi dalle ricerche recenti",
        "All" => "Tutti",
        "Tracks" => "Brani",
        "Albums" => "Album",
        "Artists" => "Artisti",
        "Playlists" => "Playlist",
        "Open" => "Apri",
        "View all" => "Mostra tutto",
        "View less" => "Mostra meno",
        "results" => "risultati",
        "found" => "trovati",
        "Loading…" => "Caricamento…",
        "Search result type" => "Tipo di risultato",
        "Start searching for music" => "Inizia a cercare musica",
        "Type an artist, song, or album in the search bar above to listen in FLAC & HQ." => {
            "Digita un artista, un brano o un album nella barra di ricerca qui sopra per ascoltarlo in FLAC e alta qualità."
        }
        "No results found" => "Nessun risultato trovato",
        "Try a different search or result category." => {
            "Prova una ricerca o una categoria diversa."
        }
        "Search Failed" => "Ricerca non riuscita",
        "About this playlist" => "Informazioni su questa playlist",
        "About this album" => "Informazioni su questo album",
        "Description" => "Descrizione",
        "Author" => "Autore",
        "Created" => "Creata",
        "Modified" => "Modificata",
        "Duration" => "Durata",
        "Title" => "Titolo",
        "Subtitle" => "Sottotitolo",
        "Release Date" => "Data di uscita",
        "Album artist" => "Artista dell'album",
        "Album type" => "Tipo di album",
        "Genre" => "Genere",
        "Explicit" => "Esplicito",
        "Yes" => "Sì",
        "No" => "No",
        "Record Label" => "Etichetta discografica",
        "Copyright" => "Copyright",
        "Barcode" => "Codice a barre",
        "Total tracks" => "Brani totali",
        "Total discs" => "Dischi totali",
        "h" => "h",
        "m" => "min",
        "s" => "s",
        "Similar Artists" => "Artisti simili",
        "Featured in" => "In evidenza in",
        "Account required" => "Account richiesto",
        "SoundCloud account required" => "Account SoundCloud richiesto",
        "Deezer account required" => "Account Deezer richiesto",
        "Log in to SoundCloud to load personalized recommendations." => {
            "Accedi a SoundCloud per caricare consigli personalizzati."
        }
        "Log in to Deezer to load personalized recommendations." => {
            "Accedi a Deezer per caricare consigli personalizzati."
        }
        "Sign in to load personalized recommendations." => {
            "Accedi per caricare consigli personalizzati."
        }
        "Log in to SoundCloud from Settings to open this result." => {
            "Accedi a SoundCloud dalle impostazioni per aprire questo risultato."
        }
        "Log in to Deezer from Settings to open this result." => {
            "Accedi a Deezer dalle impostazioni per aprire questo risultato."
        }
        "Could not open result" => "Impossibile aprire il risultato",
        "Edit" => "Modifica",
        "Play playlist" => "Riproduci playlist",
        "Favorite" => "Aggiungi ai preferiti",
        "Try again" => "Riprova",
        "Open account settings" => "Apri le impostazioni dell'account",
        "Retry" => "Riprova",
        "Discover" => "Scopri",
        "No recommendations" => "Nessun consiglio",
        "This provider did not return any Discover sections." => {
            "Questo servizio non ha restituito sezioni di Scopri."
        }
        "Try again to reload this provider." => "Riprova a ricaricare questo servizio.",
        "Retry Discover" => "Riprova a caricare Scopri",
        "Play Deezer mix for" => "Riproduci il mix Deezer basato su",
        "Play Deezer mix" => "Riproduci il mix Deezer",
        "Play Deezer Flow" => "Riproduci Deezer Flow",
        "Open Deezer channel" => "Apri il canale Deezer",
        "Open SoundCloud selection" => "Apri la selezione SoundCloud",
        "Could not open browser link" => "Impossibile aprire il link nel browser",
        "Remove from playlist" => "Rimuovi dalla playlist",
        "A SoundCloud account is required." => "È necessario un account SoundCloud.",
        "The SoundCloud library client is unavailable." => {
            "Il client della libreria SoundCloud non è disponibile."
        }
        "SoundCloud did not return playable tracks." => {
            "SoundCloud non ha restituito brani riproducibili."
        }
        "No playable tracks were returned." => "Non è stato restituito alcun brano riproducibile.",
        "Could not play SoundCloud selection" => "Impossibile riprodurre la selezione SoundCloud",
        "SoundCloud selection is empty" => "La selezione SoundCloud è vuota",
        "Log in to SoundCloud from Settings to continue." => {
            "Accedi a SoundCloud dalle impostazioni per continuare."
        }
        "AI content blocking unavailable" => "Blocco dei contenuti IA non disponibile",
        "Sign in to Deezer so ralgruM can identify AI generated tracks." => {
            "Accedi a Deezer affinché ralgruM possa identificare i brani generati dall'IA."
        }
        "Deezer and SoundCloud did not return every requested category." => {
            "Deezer e SoundCloud non hanno restituito tutte le categorie richieste."
        }
        "Deezer did not return every requested category." => {
            "Deezer non ha restituito tutte le categorie richieste."
        }
        "SoundCloud did not return every requested category." => {
            "SoundCloud non ha restituito tutte le categorie richieste."
        }
        "Playlist playback canceled" => "Riproduzione della playlist annullata",
        "Playlist import canceled" => "Importazione della playlist annullata",
        "The account changed while the playlist was loading. Try again." => {
            "L'account è cambiato durante il caricamento della playlist. Riprova."
        }
        "Playlist import failed" => "Importazione della playlist non riuscita",
        "The playlist did not load completely, so nothing was imported." => {
            "La playlist non è stata caricata completamente, quindi non è stato importato nulla."
        }
        "Playlist unavailable" => "Playlist non disponibile",
        "The playlist did not load completely, so it was not played." => {
            "La playlist non è stata caricata completamente, quindi non è stata riprodotta."
        }
        "Collection unavailable" => "Raccolta non disponibile",
        "This playlist is empty" => "Questa playlist è vuota",
        "Nothing here" => "Niente qui",
        "Add tracks to this playlist to get started." => {
            "Aggiungi brani a questa playlist per iniziare."
        }
        "This result does not contain any items." => "Questo risultato non contiene elementi.",
        "Album empty" => "Album vuoto",
        "This album has no tracks to add." => "Questo album non contiene brani da aggiungere.",
        "This album has no tracks to download." => "Questo album non contiene brani da scaricare.",
        "Playlist empty" => "Playlist vuota",
        "This playlist has no tracks to add." => {
            "Questa playlist non contiene brani da aggiungere."
        }
        "This playlist has no tracks to download." => {
            "Questa playlist non contiene brani da scaricare."
        }
        _ => return None,
    })
}
