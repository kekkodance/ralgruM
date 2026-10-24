pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "Restore" => "Ripristina",
        "Maximize" => "Ingrandisci",
        "Minimize" => "Riduci a icona",
        "Close" => "Chiudi",
        "Toggle navigation" => "Mostra o nascondi la navigazione",
        "Close navigation" => "Chiudi la navigazione",
        "Open settings" => "Apri le impostazioni",
        "Settings" => "Impostazioni",
        "Settings categories" => "Categorie delle impostazioni",
        "Back" => "Indietro",
        "Discover" => "Scopri",
        "Library" => "Libreria",
        "Downloads" => "Download",
        "Cache" => "Cache",
        "Tracks you've played before, cached for offline playback." => {
            "Brani ascoltati in precedenza, memorizzati nella cache per la riproduzione offline."
        }
        "track" => "brano",
        "tracks" => "brani",
        "Loading cache" => "Caricamento della cache",
        "Checking complete cached tracks." => "Verifica dei brani completi nella cache.",
        "No cached tracks yet" => "Nessun brano nella cache",
        "Fully cached tracks will appear here after they are played or prefetched." => {
            "I brani memorizzati completamente nella cache appariranno qui dopo la riproduzione o il precaricamento."
        }
        "Removed from cache" => "Rimosso dalla cache",
        "Could not remove from cache" => "Impossibile rimuovere dalla cache",
        "The cached track is no longer in the cache catalog." => {
            "Il brano non è più presente nel catalogo della cache."
        }
        "Cache removal request failed" => "Richiesta di rimozione dalla cache non riuscita",
        "All" => "Tutti",
        "Local" => "Locale",
        "General" => "Generale",
        "Murglar" => "Murglar",
        "Providers" => "Servizi",
        "Plugins" => "Plugin",
        "About" => "Informazioni",
        "Log Out All" => "Esci da tutti i servizi",
        "Logs out of all active services\n(Murglar, SoundCloud, Deezer)" => {
            "Disconnette tutti i servizi attivi\n(Murglar, SoundCloud, Deezer)"
        }
        "Import / Export" => "Importa / Esporta",
        "Import or Export Settings" => "Importa o esporta le impostazioni",
        "Premium:" => "Premium:",
        "Pass:" => "Piano:",
        "N/A" => "N/D",
        "Back to library" => "Torna alla libreria",
        "Back to cache" => "Torna alla cache",
        "Back to search results" => "Torna ai risultati di ricerca",
        "Back to Discover" => "Torna a Scopri",
        "Clear search" => "Cancella la ricerca",
        "Clear library search" => "Cancella la ricerca nella libreria",
        "Search" => "Cerca",
        "Search source or library service" => "Servizio di ricerca o della libreria",
        "Music search" => "Ricerca musicale",
        "Search suggestions" => "Suggerimenti di ricerca",
        "Remove from recent searches" => "Rimuovi dalle ricerche recenti",
        "Search songs, artists, or albums..." => "Cerca brani, artisti o album...",
        "Clear list" => "Svuota elenco",
        "Open folder" => "Apri cartella",
        "item" => "elemento",
        "items" => "elementi",
        "No downloads yet" => "Nessun download",
        "Downloads will appear here as soon as they are queued." => {
            "I download appariranno qui non appena saranno messi in coda."
        }
        "Queued" => "In coda",
        "Preparing download…" => "Preparazione del download…",
        "File already exists" => "Il file esiste già",
        "Downloaded" => "Scaricato",
        "downloaded" => "scaricati",
        "Skipped (existing file kept)" => "Saltato (file esistente mantenuto)",
        "Cancelled" => "Annullato",
        "Failed" => "Non riuscito",
        "Downloading" => "Download in corso",
        "Quality unavailable" => "Qualità non disponibile",
        "Show in File Explorer" => "Mostra nel gestore file",
        "Cancel queued download" => "Annulla il download in coda",
        "Cancel download" => "Annulla download",
        "Retry download" => "Riprova il download",
        "of" => "di",
        "Ignore" => "Ignora",
        "Overwrite" => "Sovrascrivi",
        "Download destinations conflict" => "Conflitto tra le destinazioni dei download",
        "Track downloaded" => "Brano scaricato",
        "Downloads finished" => "Download completati",
        "1 track downloaded." => "1 brano scaricato.",
        "tracks downloaded." => "brani scaricati.",
        "1 track conflicts with a file or another batch item. Ignore or overwrite the conflict." => {
            "1 brano è in conflitto con un file o un altro elemento del gruppo. Ignora o sovrascrivi il conflitto."
        }
        "tracks conflict with files or other batch items. Ignore or overwrite the conflicts." => {
            "brani sono in conflitto con file o altri elementi del gruppo. Ignora o sovrascrivi i conflitti."
        }
        "downloaded at" => "scaricato con qualità",
        "Cannot open the release page" => "Impossibile aprire la pagina della versione",
        "Update could not start" => "Impossibile avviare l'aggiornamento",
        "Update to" => "Aggiorna alla versione",
        "Download" => "Scarica",
        "Restart to update" => "Riavvia per aggiornare",
        "Install" => "Installa",
        "and restart" => "e riavvia",
        "Preparing update" => "Preparazione dell'aggiornamento",
        "Preparing the update helper" => "Preparazione dello strumento di aggiornamento",
        "Open release" => "Apri la versione",
        "Update failed" => "Aggiornamento non riuscito",
        "Click for the manual download" => "Fai clic per scaricare manualmente",
        _ => return None,
    })
}
