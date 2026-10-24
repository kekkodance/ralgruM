pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "My Tracks" => "I miei brani",
        "Tracks" => "Brani",
        "Artists" => "Artisti",
        "History" => "Cronologia",
        "Flow" => "Flow",
        "Station" => "Stazione",
        "Flow mode" => "Modalità Flow",
        "Mix mode" => "Modalità Mix",
        "A fresh personalized queue." => "Una nuova coda personalizzata.",
        "Local" => "Locale",
        "Local Tracks" => "Brani locali",
        "Local Playlists" => "Playlist locali",
        "Tracks saved to your local library from Deezer and SoundCloud." => {
            "Brani salvati nella libreria locale da Deezer e SoundCloud."
        }
        "Playlists organized in your local library with tracks from Deezer and SoundCloud." => {
            "Playlist organizzate nella libreria locale con brani da Deezer e SoundCloud."
        }
        "Liked Tracks" => "Brani che ti piacciono",
        "Tracks you liked on Deezer." => "Brani a cui hai messo Mi piace su Deezer.",
        "Tracks you liked on SoundCloud." => "Brani a cui hai messo Mi piace su SoundCloud.",
        "Favorite Albums" => "Album preferiti",
        "Albums you liked on Deezer." => "Album a cui hai messo Mi piace su Deezer.",
        "Followed Artists" => "Artisti seguiti",
        "Artists you followed on Deezer." => "Artisti che segui su Deezer.",
        "Playlists" => "Playlist",
        "Your playlist you saved or created on Deezer." => "Playlist salvate o create su Deezer.",
        "Listening History" => "Cronologia di ascolto",
        "Your most recently played Deezer tracks." => "I brani riprodotti di recente su Deezer.",
        "Personalized moods and genres for you." => "Mood e generi personalizzati per te.",
        "Tracks uploaded on your SoundCloud account." => {
            "Brani caricati sul tuo account SoundCloud."
        }
        "Albums" => "Album",
        "Albums you liked on SoundCloud." => "Album a cui hai messo Mi piace su SoundCloud.",
        "Artists you followed on SoundCloud." => "Artisti che segui su SoundCloud.",
        "Your playlist you saved or created on SoundCloud." => {
            "Playlist salvate o create su SoundCloud."
        }
        "Track History" => "Cronologia brani",
        "Your most recently played SoundCloud tracks." => {
            "I brani riprodotti di recente su SoundCloud."
        }
        "Start a continuous mix from one of your liked tracks." => {
            "Avvia un mix continuo da uno dei brani a cui hai messo Mi piace."
        }
        "Nothing here yet" => "Qui non c'è ancora nulla",
        "Tracks uploaded by your SoundCloud account will appear here." => {
            "Qui appariranno i brani caricati dal tuo account SoundCloud."
        }
        "No local tracks yet" => "Non ci sono ancora brani locali",
        "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file." => {
            "Qui appariranno i brani salvati nella libreria locale. Salvare un brano conserva un riferimento, non il file audio."
        }
        "This part of your library is currently empty." => "Questa sezione della libreria è vuota.",
        "Flow is unavailable" => "Flow non è disponibile",
        "No Flow mixes were returned for this account." => {
            "Questo account non ha ricevuto mix Flow."
        }
        "No saved albums" => "Non ci sono album salvati",
        "No favorite albums" => "Non ci sono album preferiti",
        "Albums you save on SoundCloud will appear here." => {
            "Qui appariranno gli album salvati su SoundCloud."
        }
        "Albums you save on Deezer will appear here." => {
            "Qui appariranno gli album salvati su Deezer."
        }
        "No followed artists" => "Non ci sono artisti seguiti",
        "Artists you follow on SoundCloud will appear here." => {
            "Qui appariranno gli artisti che segui su SoundCloud."
        }
        "Artists you follow on Deezer will appear here." => {
            "Qui appariranno gli artisti che segui su Deezer."
        }
        "No local playlists yet" => "Non ci sono ancora playlist locali",
        "Create a local playlist to organize track references. Local playlists do not download audio files." => {
            "Crea una playlist locale per organizzare i riferimenti ai brani. Le playlist locali non scaricano i file audio."
        }
        "No playlists" => "Nessuna playlist",
        "Playlists you create or save on SoundCloud will appear here." => {
            "Qui appariranno le playlist create o salvate su SoundCloud."
        }
        "Saved and created Deezer playlists will appear here." => {
            "Qui appariranno le playlist salvate o create su Deezer."
        }
        "No station seeds" => "Nessun brano di partenza per la stazione",
        "Like a SoundCloud track first, then use it to start a station." => {
            "Metti Mi piace a un brano SoundCloud e usalo per avviare una stazione."
        }
        "No matches on this page" => "Nessun risultato in questa pagina",
        "Nothing here matches your search query." => "Nessun elemento corrisponde alla ricerca.",
        "This local playlist is empty" => "Questa playlist locale è vuota",
        "Create a local playlist to organize tracks from all platforms." => {
            "Crea una playlist locale per organizzare brani da tutte le piattaforme."
        }
        "No items in this section." => "Nessun elemento in questa sezione.",
        "Similar Artists" => "Artisti simili",
        "Artists related to" => "Artisti simili a",
        "Loading similar artists" => "Caricamento artisti simili",
        "Deezer is finding related artists." => "Deezer sta cercando artisti simili.",
        "No similar artists" => "Nessun artista simile",
        "Deezer did not return related artists for this selection." => {
            "Deezer non ha trovato artisti simili per questa selezione."
        }
        "Could not load similar artists" => "Impossibile caricare gli artisti simili",
        "Deezer did not return a related artist list." => {
            "Deezer non ha restituito un elenco di artisti simili."
        }
        "Search this library page..." => "Cerca in questa pagina della libreria...",
        "Library category" => "Categoria della libreria",
        "Play liked tracks" => "Riproduci i brani preferiti",
        "Play playlist" => "Riproduci playlist",
        "Open account settings" => "Apri le impostazioni dell'account",
        "Play" => "Riproduci",
        "Open" => "Apri",
        "Remove from playlist" => "Rimuovi dalla playlist",
        "Delete local playlist" => "Elimina playlist locale",
        "Loading playlists..." => "Caricamento playlist...",
        "No Local playlists yet" => "Non ci sono ancora playlist locali",
        "No editable playlists" => "Nessuna playlist modificabile",
        "Create a Local playlist to get started." => "Crea una playlist locale per iniziare.",
        "Create a new playlist to get started." => "Crea una nuova playlist per iniziare.",
        "No matching playlists" => "Nessuna playlist corrispondente",
        "Try a different filter." => "Prova un filtro diverso.",
        "Create Playlist" => "Crea playlist",
        "Retry" => "Riprova",
        "Cancel" => "Annulla",
        "Adding..." => "Aggiunta...",
        "Add track" => "Aggiungi brano",
        "Add to Local playlist" => "Aggiungi alla playlist locale",
        "Add to Deezer playlist" => "Aggiungi alla playlist Deezer",
        "Add to SoundCloud playlist" => "Aggiungi alla playlist SoundCloud",
        "Private playlist" => "Playlist privata",
        "Private" => "Privata",
        "Public" => "Pubblica",
        "Local playlist" => "Playlist locale",
        "Change cover image" => "Cambia immagine di copertina",
        "Choose cover image" => "Scegli immagine di copertina",
        "Title" => "Titolo",
        "Description" => "Descrizione",
        "What is this playlist called?" => "Come si chiama questa playlist?",
        "What is this playlist about?" => "Di cosa parla questa playlist?",
        "Enter a playlist title." => "Inserisci il titolo della playlist.",
        "Playlist title is invalid" => "Il titolo della playlist non è valido",
        "Playlist descriptions can contain at most 4000 characters." => {
            "La descrizione della playlist può contenere al massimo 4000 caratteri."
        }
        "Playlist descriptions can contain at most 200 characters." => {
            "La descrizione della playlist può contenere al massimo 200 caratteri."
        }
        "SoundCloud playlist descriptions can contain at most 4000 characters." => {
            "La descrizione della playlist SoundCloud può contenere al massimo 4000 caratteri."
        }
        "Playlist descriptions cannot contain unsupported control characters." => {
            "La descrizione della playlist non può contenere caratteri di controllo non supportati."
        }
        "Cover image" => "Immagine di copertina",
        "Drag to reposition. Scroll or use the slider to zoom." => {
            "Trascina per riposizionare. Scorri o usa il cursore per ingrandire."
        }
        "Cover crop. Use the arrow keys or drag to reposition the image" => {
            "Ritaglio copertina. Usa i tasti freccia o trascina per riposizionare l'immagine"
        }
        "Cover image zoom" => "Zoom immagine di copertina",
        "Edit Deezer playlist" => "Modifica playlist Deezer",
        "Edit SoundCloud playlist" => "Modifica playlist SoundCloud",
        "Delete Deezer playlist" => "Elimina playlist Deezer",
        "Delete SoundCloud playlist" => "Elimina playlist SoundCloud",
        "Saving..." => "Salvataggio...",
        "Save changes" => "Salva modifiche",
        "Creating..." => "Creazione...",
        "Deleting..." => "Eliminazione...",
        "Retrying..." => "Nuovo tentativo...",
        "Retry adding track" => "Riprova ad aggiungere il brano",
        "Delete playlist" => "Elimina playlist",
        "Delete" => "Elimina",
        "from" => "da",
        "from your Local library" => "dalla libreria locale",
        "This cannot be undone." => "Questa operazione non può essere annullata.",
        "This playlist will start with" => "Questa playlist inizierà con",
        "selected track" => "brano selezionato",
        "selected tracks" => "brani selezionati",
        "Delete \"{}\" from {}? This cannot be undone." => {
            "Eliminare \"{}\" da {}? Questa operazione non può essere annullata."
        }
        "Track info" => "Informazioni sul brano",
        "Subtitle" => "Sottotitolo",
        "Release Date" => "Data di pubblicazione",
        "Album artist" => "Artista dell'album",
        "Album type" => "Tipo di album",
        "Genre" => "Genere",
        "Explicit" => "Contenuto esplicito",
        "Yes" => "Sì",
        "No" => "No",
        "Composer" => "Compositore",
        "Author" => "Autore",
        "Writer" => "Scrittore",
        "Record Label" => "Etichetta discografica",
        "Copyright" => "Copyright",
        "ISRC" => "ISRC",
        "Barcode" => "Codice a barre",
        "Track number" => "Numero del brano",
        "Total tracks" => "Brani totali",
        "Disc number" => "Numero del disco",
        "Total discs" => "Dischi totali",
        "Duration" => "Durata",
        "BPM" => "BPM",
        "Gain" => "Guadagno",
        "Peak" => "Picco",
        "Size unavailable" => "Dimensione non disponibile",
        "track" => "brano",
        "tracks" => "brani",
        "item" => "elemento",
        "items" => "elementi",
        "{} track" => "{} brano",
        "{} tracks" => "{} brani",
        "{} item" => "{} elemento",
        "{} items" => "{} elementi",
        "Your new playlist is ready." => "La tua nuova playlist è pronta.",
        "Your track was added to the new playlist." => {
            "Il brano è stato aggiunto alla nuova playlist."
        }
        "Your changes were saved." => "Le modifiche sono state salvate.",
        "The playlist was removed from your Local library." => {
            "La playlist è stata rimossa dalla libreria locale."
        }
        "Local playlist created" => "Playlist locale creata",
        "Local playlist updated" => "Playlist locale aggiornata",
        "Added to local playlist" => "Aggiunto alla playlist locale",
        "Already in local playlist" => "Già presente nella playlist locale",
        "Local playlist deleted" => "Playlist locale eliminata",
        "Track removed from local playlist" => "Brano rimosso dalla playlist locale",
        "Playlist created" => "Playlist creata",
        "Favorite failed" => "Operazione Mi piace non riuscita",
        "Playlist import failed" => "Importazione playlist non riuscita",
        "Playlist unavailable" => "Playlist non disponibile",
        "Saved to local" => "Salvato in locale",
        "Removed from local" => "Rimosso dalla libreria locale",
        "Local library could not be updated" => "Impossibile aggiornare la libreria locale",
        "No tracks to play" => "Nessun brano da riprodurre",
        "Liked tracks are still loading" => "I brani preferiti sono ancora in caricamento",
        "No liked tracks to play" => "Nessun brano preferito da riprodurre",
        "No playable tracks were returned." => "Non sono stati restituiti brani riproducibili.",
        "No playable tracks were found for" => "Non sono stati trovati brani riproducibili per",
        "A SoundCloud account is required." => "È necessario un account SoundCloud.",
        "A Deezer account is required." => "È necessario un account Deezer.",
        "The search client is unavailable." => "Il client di ricerca non è disponibile.",
        "The SoundCloud library client is unavailable." => {
            "Il client della libreria SoundCloud non è disponibile."
        }
        "The Deezer library client is unavailable." => {
            "Il client della libreria Deezer non è disponibile."
        }
        "Try again after the liked tracks finish loading." => {
            "Riprova quando il caricamento dei brani preferiti sarà terminato."
        }
        "Wait for the full list to finish loading, then try again." => {
            "Attendi il caricamento dell'elenco completo e riprova."
        }
        "SoundCloud did not return a playable station." => {
            "SoundCloud non ha restituito una stazione riproducibile."
        }
        "Could not start SoundCloud station" => "Impossibile avviare la stazione SoundCloud",
        "SoundCloud station is empty" => "La stazione SoundCloud è vuota",
        "Could not update recommendations" => "Impossibile aggiornare i consigli",
        "Deezer will show fewer similar recommendations." => {
            "Deezer mostrerà meno consigli simili."
        }
        "Deezer did not confirm the recommendation change." => {
            "Deezer non ha confermato la modifica dei consigli."
        }
        "Recommendation updated" => "Consiglio aggiornato",
        "Could not start Deezer mix" => "Impossibile avviare il mix Deezer",
        "Deezer did not return a playable mix." => "Deezer non ha restituito un mix riproducibile.",
        "Deezer mix is empty" => "Il mix Deezer è vuoto",
        "Could not start Deezer Flow" => "Impossibile avviare Deezer Flow",
        "The local playlist could not be found." => "Impossibile trovare la playlist locale.",
        "Local playlist storage is unavailable" => {
            "Lo spazio di archiviazione delle playlist locali non è disponibile"
        }
        "Local playlist could not be opened for editing" => {
            "Impossibile aprire la playlist locale per modificarla"
        }
        "Local playlist could not be found" => "Impossibile trovare la playlist locale",
        "Local playlist could not be updated" => "Impossibile aggiornare la playlist locale",
        "Track is no longer in this local playlist" => {
            "Il brano non è più presente in questa playlist locale"
        }
        "Local playlist storage is unavailable." => {
            "Lo spazio di archiviazione delle playlist locali non è disponibile."
        }
        "Local playlist storage could not be accessed." => {
            "Impossibile accedere allo spazio di archiviazione delle playlist locali."
        }
        "The local playlist file is invalid." => "Il file della playlist locale non è valido.",
        "The local playlist contains invalid data." => {
            "La playlist locale contiene dati non validi."
        }
        "The local playlist contains duplicate items." => {
            "La playlist locale contiene elementi duplicati."
        }
        "The local playlist could not be prepared." => "Impossibile preparare la playlist locale.",
        "Playlist is empty" => "La playlist è vuota",
        "Add tracks before adding this playlist to the queue." => {
            "Aggiungi brani alla playlist prima di metterla in coda."
        }
        "Add tracks before playing it." => "Aggiungi brani prima di riprodurla.",
        "Add tracks before downloading this playlist." => {
            "Aggiungi brani prima di scaricare questa playlist."
        }
        "Download unavailable" => "Download non disponibile",
        "One or more tracks cannot be downloaded with the connected accounts." => {
            "Non è possibile scaricare uno o più brani con gli account collegati."
        }
        "The Local playlist could not be found." => "Impossibile trovare la playlist locale.",
        "Collection unavailable" => "Raccolta non disponibile",
        "The playlist did not load completely, so nothing was imported." => {
            "La playlist non è stata caricata completamente, quindi non è stato importato nulla."
        }
        "The playlist did not load completely, so it was not played." => {
            "La playlist non è stata caricata completamente, quindi non è stata riprodotta."
        }
        "Local playlist could not be found." => "Impossibile trovare la playlist locale.",
        "The cover image chooser failed." => {
            "Impossibile aprire la selezione dell'immagine di copertina."
        }
        "The dropped cover image could not be loaded." => {
            "Impossibile caricare l'immagine di copertina trascinata."
        }
        "The initial track retry is no longer available." => {
            "Non è più possibile riprovare ad aggiungere il brano iniziale."
        }
        "Choose and crop a playlist cover." => {
            "Scegli e ritaglia un'immagine di copertina per la playlist."
        }
        "The playlist cover could not be imported." => {
            "Impossibile importare la copertina della playlist."
        }
        "Empty playlist imported." => "Playlist vuota importata.",
        "Repeated tracks were collapsed." => "I brani ripetuti sono stati accorpati.",
        "track imported" => "brano importato",
        "tracks imported" => "brani importati",
        "unique track imported" => "brano unico importato",
        "unique tracks imported" => "brani unici importati",
        "Imported to Local" => "Importata in locale",
        "Tracks were already in the playlist" => "brani erano già nella playlist",
        "Added" => "Aggiunti",
        "were already in the playlist" => "erano già nella playlist",
        "Track added to playlist." => "Brano aggiunto alla playlist.",
        "tracks added to playlist." => "brani aggiunti alla playlist.",
        "Track was already in that playlist." => "Il brano era già presente nella playlist.",
        "Those tracks were already in that playlist." => {
            "Quei brani erano già presenti nella playlist."
        }
        "Track added to" => "Brano aggiunto a",
        "tracks added to" => "brani aggiunti a",
        "Added to playlist" => "Aggiunto alla playlist",
        "Track added" => "Brano aggiunto",
        "track added" => "brano aggiunto",
        "tracks added" => "brani aggiunti",
        "Collection request failed" => "Richiesta della raccolta non riuscita",
        "The Deezer account changed while this dialog was open." => {
            "L'account Deezer è cambiato mentre questa finestra era aperta."
        }
        "The SoundCloud account changed while this dialog was open." => {
            "L'account SoundCloud è cambiato mentre questa finestra era aperta."
        }
        "The Deezer account changed while deletion was in progress." => {
            "L'account Deezer è cambiato durante l'eliminazione."
        }
        "The SoundCloud account changed while deletion was in progress." => {
            "L'account SoundCloud è cambiato durante l'eliminazione."
        }
        "The Deezer account changed while the cover image was loading." => {
            "L'account Deezer è cambiato durante il caricamento della copertina."
        }
        "The SoundCloud account changed while the cover image was loading." => {
            "L'account SoundCloud è cambiato durante il caricamento della copertina."
        }
        "The Deezer account changed while the playlist update was in progress." => {
            "L'account Deezer è cambiato durante l'aggiornamento della playlist."
        }
        "The SoundCloud account changed while the playlist update was in progress." => {
            "L'account SoundCloud è cambiato durante l'aggiornamento della playlist."
        }
        "SoundCloud account required" => "È necessario un account SoundCloud",
        "Deezer account required" => "È necessario un account Deezer",
        "Account required" => "È necessario un account",
        "Log in to SoundCloud to load this part of your personal library." => {
            "Accedi a SoundCloud per caricare questa sezione della tua libreria personale."
        }
        "Log in to Deezer to load this part of your personal library." => {
            "Accedi a Deezer per caricare questa sezione della tua libreria personale."
        }
        "Sign in to load this part of your personal library." => {
            "Accedi per caricare questa sezione della tua libreria personale."
        }
        "Drop one image file at a time." => "Trascina un solo file immagine alla volta.",
        "Choose a supported image file." => "Scegli un file immagine supportato.",
        "The selected playlist is no longer editable." => {
            "La playlist selezionata non è più modificabile."
        }
        "The SoundCloud account changed while this picker was open." => {
            "L'account SoundCloud è cambiato mentre questo selettore era aperto."
        }
        "Deezer login required" => "È necessario accedere a Deezer",
        "Deezer playlist client could not be created" => {
            "Impossibile creare il client playlist Deezer"
        }
        "SoundCloud login required" => "È necessario accedere a SoundCloud",
        "SoundCloud playlist client could not be created" => {
            "Impossibile creare il client playlist SoundCloud"
        }
        "This Deezer playlist is no longer editable." => {
            "Questa playlist Deezer non è più modificabile."
        }
        "This SoundCloud playlist is no longer editable." => {
            "Questa playlist SoundCloud non è più modificabile."
        }
        "The Deezer account changed while this playlist editor was open." => {
            "L'account Deezer è cambiato mentre questo editor playlist era aperto."
        }
        "The SoundCloud account changed while this playlist editor was open." => {
            "L'account SoundCloud è cambiato mentre questo editor playlist era aperto."
        }
        "Added {} tracks; {} were already in the playlist." => {
            "Aggiunti {} brani; {} erano già nella playlist."
        }
        _ => return None,
    })
}
