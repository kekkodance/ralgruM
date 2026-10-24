pub(super) fn translate(source: &str) -> Option<&'static str> {
    Some(match source {
        "Settings" => "Impostazioni",
        "General" => "Generale",
        "Language" => "Lingua",
        "App language" => "Lingua dell'app",
        "Choose the language used throughout ralgruM." => "Scegli la lingua da usare in ralgruM.",
        "Choose the app language." => "Scegli la lingua dell'app.",
        "App behavior" => "Comportamento dell'app",
        "Playback" => "Riproduzione",
        "Cache & storage" => "Cache e archiviazione",
        "Start page" => "Pagina iniziale",
        "Choose what appears when ralgruM opens." => {
            "Scegli cosa mostrare all'apertura di ralgruM."
        }
        "Remember navigation choices" => "Ricorda le scelte di navigazione",
        "Restore search filters and each library category." => {
            "Ripristina i filtri di ricerca e le categorie della libreria."
        }
        "Restore window" => "Ripristina la finestra",
        "Save this preference for future window restoration." => {
            "Salva questa preferenza per ripristinare la finestra in futuro."
        }
        "Keep running in system tray" => "Mantieni l'app nell'area di notifica",
        "Hide the window when it is closed and keep playback running in the tray." => {
            "Nascondi la finestra alla chiusura e continua la riproduzione nell'area di notifica."
        }
        "Disable animations" => "Disattiva le animazioni",
        "Turn off interface animations and motion effects." => {
            "Disattiva le animazioni e gli effetti di movimento dell'interfaccia."
        }
        "Disable explicit content" => "Disattiva i contenuti espliciti",
        "Keep explicit tracks visible, but gray them out and skip them everywhere." => {
            "Mantieni visibili i brani espliciti, ma attenuali e saltali ovunque."
        }
        "Disable AI generated content" => "Disattiva i contenuti generati dall'IA",
        "Keep AI generated tracks visible, but gray them out and skip them everywhere." => {
            "Mantieni visibili i brani generati dall'IA, ma attenuali e saltali ovunque."
        }
        "Audio output" => "Uscita audio",
        "Choose how ralgruM starts, plays, and uses local storage." => {
            "Scegli come ralgruM si avvia, riproduce e usa l'archiviazione locale."
        }
        "Choose where ralgruM plays audio." => "Scegli dove ralgruM riproduce l'audio.",
        "Output device" => "Dispositivo di uscita",
        "Select the device used for playback." => {
            "Seleziona il dispositivo usato per la riproduzione."
        }
        "ASIO mode" => "Modalità ASIO",
        "Use a low-latency ASIO output when available." => {
            "Usa un'uscita ASIO a bassa latenza, se disponibile."
        }
        "Lyrics source" => "Fonte dei testi",
        "Choose where synchronized lyrics are loaded from." => {
            "Scegli da dove caricare i testi sincronizzati."
        }
        "Remember playback modes" => "Ricorda le modalità di riproduzione",
        "Restore shuffle and repeat when ralgruM opens." => {
            "Ripristina casuale e ripetizione all'apertura di ralgruM."
        }
        "Seamless playback" => "Riproduzione senza interruzioni",
        "Reduce the gap between tracks." => "Riduci la pausa tra i brani.",
        "Seamless transitions" => "Transizioni senza interruzioni",
        "Preload the next track near the end for a clean handoff." => {
            "Precarica il brano successivo prima della fine per una transizione fluida."
        }
        "Remember shuffle and repeat" => "Ricorda casuale e ripetizione",
        "Keep both playback modes after restarting." => {
            "Mantieni entrambe le modalità di riproduzione dopo il riavvio."
        }
        "Default lyrics provider" => "Servizio predefinito per i testi",
        "You can still switch providers from the lyrics pane." => {
            "Puoi comunque cambiare servizio dal pannello dei testi."
        }
        "Share activity with Discord" => "Condividi l'attività con Discord",
        "Show the current track in Discord Rich Presence." => {
            "Mostra il brano attuale nello stato Discord Rich Presence."
        }
        "Cache tracks in the background" => "Memorizza i brani in cache in background",
        "Finish caching a track after playback begins." => {
            "Completa la memorizzazione del brano dopo l'avvio della riproduzione."
        }
        "Audio cache size" => "Dimensione della cache audio",
        "Older cached audio is removed automatically." => {
            "I file audio meno recenti vengono rimossi automaticamente dalla cache."
        }
        "Downloads folder" => "Cartella dei download",
        "Change folder" => "Cambia cartella",
        "Choose a different downloads folder" => "Scegli un'altra cartella per i download",
        "Reset" => "Ripristina",
        "Use the system downloads folder" => "Usa la cartella Download di sistema",
        "Open Downloads" => "Apri i download",
        "Open the downloads folder" => "Apri la cartella dei download",
        "Clear cache" => "Svuota la cache",
        "Clear cached audio" => "Elimina l'audio memorizzato nella cache",
        "Audio cache" => "Cache audio",
        "Calculating..." => "Calcolo in corso...",
        "of" => "di",
        "used" => "utilizzati",
        "Last used" => "Ultima usata",
        "Account profile" => "Profilo dell'account",
        "Referral rewards" => "Premi per i referral",
        "Share your referral code and track the reward days credited by Murglar." => {
            "Condividi il tuo codice referral e controlla i giorni premio accreditati da Murglar."
        }
        "Reload referral rewards" => "Ricarica i premi referral",
        "Copy referral code" => "Copia il codice referral",
        "Reward days" => "Giorni premio",
        "Invitees" => "Invitati",
        "Rewards" => "Premi",
        "Special Thanks" => "Ringraziamenti",
        ", for making it possible" => ", per aver reso tutto questo possibile",
        ", for testing" => ", per i test",
        ", for the app icon" => ", per l'icona dell'app",
        "Third-party notices" => "Note sui componenti di terze parti",
        "No plugins yet" => "Nessun plugin",
        "Integrations and Extensions built into ralgruM." => {
            "Integrazioni ed estensioni incluse in ralgruM."
        }
        "Built-in integrations will appear here when they are added to ralgruM." => {
            "Le integrazioni incluse appariranno qui quando verranno aggiunte a ralgruM."
        }
        "Open plugin settings" => "Apri le impostazioni del plugin",
        "Enable the plugin to change its settings" => {
            "Attiva il plugin per modificarne le impostazioni"
        }
        "Open plugin UI" => "Apri l'interfaccia del plugin",
        "Enable the plugin to open its UI" => "Attiva il plugin per aprirne l'interfaccia",
        "Plugin could not be enabled" => "Impossibile attivare il plugin",
        "Plugin could not be disabled" => "Impossibile disattivare il plugin",
        "Plugin storage is unavailable." => "Archiviazione dei plugin non disponibile.",
        "Settings storage is unavailable." => "Archiviazione delle impostazioni non disponibile.",
        "The settings writer stopped unexpectedly." => {
            "Il processo di salvataggio delle impostazioni si è interrotto inaspettatamente."
        }
        "Sign out of all services?" => "Vuoi uscire da tutti i servizi?",
        "Log out of all services?" => "Vuoi uscire da tutti i servizi?",
        "Log Out All" => "Esci da tutti",
        "Import / Export" => "Importa / Esporta",
        "Import settings" => "Importa impostazioni",
        "Export settings" => "Esporta impostazioni",
        "Choose a settings file to import." => "Scegli un file di impostazioni da importare.",
        "Export settings to a file." => "Esporta le impostazioni in un file.",
        "Dismiss notification" => "Chiudi la notifica",
        "No Local playlists yet" => "Nessuna playlist locale",
        "No matching playlists" => "Nessuna playlist corrispondente",
        "Try a different filter." => "Prova un filtro diverso.",
        "Your Deezer session has expired" => "La sessione Deezer è scaduta",
        "Log in to Deezer again to refresh your library." => {
            "Accedi di nuovo a Deezer per aggiornare la libreria."
        }
        "Could not load this library" => "Impossibile caricare questa libreria",
        "Try again" => "Riprova",
        "Album empty" => "Album vuoto",
        "Playlist empty" => "Playlist vuota",
        "This album has no tracks to add." => "Questo album non contiene brani da aggiungere.",
        "This album has no tracks to download." => "Questo album non contiene brani da scaricare.",
        "This playlist has no tracks to add." => {
            "Questa playlist non contiene brani da aggiungere."
        }
        "This playlist has no tracks to download." => {
            "Questa playlist non contiene brani da scaricare."
        }
        "This playlist is empty" => "Questa playlist è vuota",
        "Nothing here" => "Niente qui",
        "Add tracks to this playlist to get started." => {
            "Aggiungi brani a questa playlist per iniziare."
        }
        "This result does not contain any items." => "Questo risultato non contiene elementi.",
        "Start searching for music" => "Inizia a cercare musica",
        "Type an artist, song, or album in the search bar above to listen in FLAC & HQ." => {
            "Cerca un artista, un brano o un album nella barra qui sopra per ascoltare in FLAC e alta qualità."
        }
        "Service status" => "Stato del servizio",
        "Open Murglar service status" => "Apri lo stato del servizio Murglar",
        "Account login" => "Accesso all'account",
        "Username or email" => "Nome utente o email",
        "Password" => "Password",
        "Who are you?" => "Chi sei?",
        "What's the code?" => "Qual è il codice?",
        "Log In to Murglar" => "Accedi a Murglar",
        "Murglar login required" => "Accesso a Murglar richiesto",
        "Log in to Murglar before choosing a payment method." => {
            "Accedi a Murglar prima di scegliere un metodo di pagamento."
        }
        "Payment page opened" => "Pagina di pagamento aperta",
        "Continue securely in your browser." => "Continua in sicurezza nel browser.",
        "Could not open payment" => "Impossibile aprire il pagamento",
        "Referral code copied" => "Codice referral copiato",
        "The Murglar referral code was copied to the clipboard." => {
            "Il codice referral Murglar è stato copiato negli appunti."
        }
        "Murglar Account" => "Account Murglar",
        "Sign in and review your subscription status." => {
            "Accedi e controlla lo stato del tuo abbonamento."
        }
        "Murglar device limit reached" => "Limite di dispositivi Murglar raggiunto",
        "Murglar Pass limit reached" => "Limite Murglar Pass raggiunto",
        "Murglar profile" => "Profilo Murglar",
        "Permanent Premium" => "Premium permanente",
        "Pass expiration" => "Scadenza del Pass",
        "Reload" => "Ricarica",
        "Copy" => "Copia",
        "Discover" => "Scopri",
        "Library" => "Libreria",
        "Downloads" => "Download",
        "About" => "Informazioni",
        "Providers" => "Servizi",
        "Plugins" => "Plugin",
        "Murglar" => "Murglar",
        "Local" => "Locale",
        "Tracks" => "Brani",
        "Albums" => "Album",
        "Artists" => "Artisti",
        "Playlists" => "Playlist",
        "Liked tracks" => "Brani piaciuti",
        "History" => "Cronologia",
        "Flow" => "Flow",
        "Station" => "Stazione",
        "Users" => "Utenti",
        "Search" => "Cerca",
        "Search tracks, artists, albums..." => "Cerca brani, artisti, album...",
        "Back" => "Indietro",
        "Next" => "Avanti",
        "Previous" => "Precedente",
        "Play" => "Riproduci",
        "Pause" => "Pausa",
        "Shuffle" => "Casuale",
        "Repeat" => "Ripeti",
        "Mute" => "Disattiva audio",
        "Unmute" => "Riattiva audio",
        "Play now" => "Riproduci ora",
        "Play next" => "Riproduci dopo",
        "Add to queue" => "Aggiungi alla coda",
        "Remove from queue" => "Rimuovi dalla coda",
        "Add to playlist" => "Aggiungi alla playlist",
        "Create playlist" => "Crea playlist",
        "Edit playlist" => "Modifica playlist",
        "Delete playlist" => "Elimina playlist",
        "Save" => "Salva",
        "Cancel" => "Annulla",
        "Close" => "Chiudi",
        "Create" => "Crea",
        "Edit" => "Modifica",
        "Delete" => "Elimina",
        "Remove" => "Rimuovi",
        "Clear" => "Svuota",
        "Copy link" => "Copia link",
        "Open in browser" => "Apri nel browser",
        "Track info" => "Informazioni sul brano",
        "Go to artist" => "Vai all'artista",
        "Go to album" => "Vai all'album",
        "Loading..." => "Caricamento...",
        "Loading account profile..." => "Caricamento del profilo...",
        "Sign in" => "Accedi",
        "Log out" => "Esci",
        "Validate and save" => "Verifica e salva",
        "Validate and save this provider session" => "Verifica e salva la sessione del servizio",
        "A secure window will open. Sign in there and ralgruM will save the session automatically." => {
            "Si aprirà una finestra sicura. Accedi lì e ralgruM salverà automaticamente la sessione."
        }
        "Enter the required session credentials below to validate and save the account." => {
            "Inserisci qui sotto le credenziali richieste per verificare e salvare l'account."
        }
        "Profile" => "Profilo",
        "Listen history" => "Cronologia di ascolto",
        "Record Deezer plays" => "Registra gli ascolti su Deezer",
        "Search suggestions" => "Suggerimenti di ricerca",
        "Enable SoundCloud search suggestions." => {
            "Abilita i suggerimenti di ricerca di SoundCloud."
        }
        "Account required" => "Account richiesto",
        "SoundCloud account required" => "Account SoundCloud richiesto",
        "Deezer account required" => "Account Deezer richiesto",
        "Sign in to load this part of your personal library." => {
            "Accedi per caricare questa sezione della tua libreria personale."
        }
        "Log in to SoundCloud to load this part of your personal library." => {
            "Accedi a SoundCloud per caricare questa sezione della tua libreria personale."
        }
        "Log in to Deezer to load this part of your personal library." => {
            "Accedi a Deezer per caricare questa sezione della tua libreria personale."
        }
        "Log in to SoundCloud to load personalized recommendations." => {
            "Accedi a SoundCloud per caricare i consigli personalizzati."
        }
        "Log in to Deezer to load personalized recommendations." => {
            "Accedi a Deezer per caricare i consigli personalizzati."
        }
        "Account" => "Account",
        "The profile task failed: {}" => "Caricamento del profilo non riuscito: {}",
        "Could not" => "Impossibile",
        "No playable tracks were found." => "Non sono stati trovati brani riproducibili.",
        "No results found." => "Nessun risultato trovato.",
        "Retry" => "Riprova",
        "Yes" => "Sì",
        "No" => "No",
        "Welcome" => "Benvenuto",
        "Could not load" => "Impossibile caricare",
        "Could not save" => "Impossibile salvare",
        "Please wait..." => "Attendi...",
        "Open" => "Apri",
        "Choose" => "Scegli",
        "Select" => "Seleziona",
        "All" => "Tutti",
        "None" => "Nessuno",
        "On" => "Attivo",
        "Off" => "Disattivo",
        "Access your library, Flow, and downloads." => {
            "Accedi alla tua libreria, a Flow e ai download."
        }
        "Access your library, original-quality playback, and search suggestions." => {
            "Accedi alla tua libreria, alla riproduzione in qualità originale e ai suggerimenti di ricerca."
        }
        "Save listening history" => "Salva la cronologia di ascolto",
        "Notify Deezer about the tracks you're playing to improve recommendations and listening history." => {
            "Comunica a Deezer i brani che ascolti per migliorare i consigli e la cronologia."
        }
        "Use SoundCloud's Search API to provide search suggestions. This works without a SoundCloud account too." => {
            "Usa l'API di ricerca di SoundCloud per mostrare suggerimenti. Funziona anche senza un account SoundCloud."
        }
        "ARL cookie" => "Cookie ARL",
        "User ID (optional, checked when provided)" => "ID utente (facoltativo)",
        "Desktop OAuth token" => "Token OAuth desktop",
        "Mobile OAuth token" => "Token OAuth mobile",
        "Paste Deezer ARL cookie" => "Incolla il cookie ARL di Deezer",
        "Optional Deezer user ID" => "ID utente Deezer facoltativo",
        "Paste desktop OAuth token" => "Incolla il token OAuth desktop",
        "Optional mobile OAuth token" => "Token OAuth mobile facoltativo",
        "The account could not be saved." => "Impossibile salvare l'account.",
        "The account could not be signed out." => "Impossibile uscire dall'account.",
        "The account session could not be stored." => {
            "Impossibile salvare la sessione dell'account."
        }
        "Signed out." => "Disconnessione completata.",
        "All accounts are signed out." => "Tutti gli account sono disconnessi.",
        "The account sessions could not be cleared." => {
            "Impossibile cancellare le sessioni degli account."
        }
        "Another sign-in is still finishing. Try again in a moment." => {
            "È ancora in corso un altro accesso. Riprova tra poco."
        }
        "Referral details are temporarily unavailable." => {
            "I dettagli dei referral non sono al momento disponibili."
        }
        "Murglar plans are temporarily unavailable." => {
            "I piani Murglar non sono al momento disponibili."
        }
        "Not signed in" => "Non hai effettuato l'accesso",
        "Murglar user" => "Utente Murglar",
        "The account profile could not be loaded." => {
            "Impossibile caricare il profilo dell'account."
        }
        "Scroll left" => "Scorri a sinistra",
        "Scroll right" => "Scorri a destra",
        "Private" => "Privato",
        "Public" => "Pubblico",
        "More options" => "Altre opzioni",
        "AI generated" => "Generato dall'IA",
        "SoundCloud Go+ track" => "Brano SoundCloud Go+",
        "Action" => "Azione",
        "Enter your username and password." => "Inserisci il nome utente e la password.",
        "Murglar account login" => "Accesso all'account Murglar",
        "Your account already has five registered devices. Contact the Murglar developers to resolve the device registrations, then retry. ralgruM will keep using its existing saved device identity." => {
            "Il tuo account ha già cinque dispositivi registrati. Contatta gli sviluppatori di Murglar per risolvere il problema, poi riprova. ralgruM continuerà a usare l'identità del dispositivo già salvata."
        }
        "Refreshing Murglar profile..." => "Aggiornamento del profilo Murglar...",
        "Murglar reports that this account has reached its current Pass usage limit. High-quality or blocked-track requests may be rejected." => {
            "Murglar segnala che questo account ha raggiunto il limite attuale di utilizzo del Pass. Le richieste di alta qualità o per brani bloccati potrebbero essere rifiutate."
        }
        "Loading referral details…" => "Caricamento dei dettagli referral…",
        "Loading available plans…" => "Caricamento dei piani disponibili…",
        "Saved Murglar session" => "Sessione Murglar salvata",
        "Pass Active" => "Pass attivo",
        "No Active Pass" => "Nessun Pass attivo",
        "Premium and Pass plans" => "Piani Premium e Pass",
        "Prices and payment methods loaded directly from Murglar." => {
            "Prezzi e metodi di pagamento caricati direttamente da Murglar."
        }
        "Price unavailable" => "Prezzo non disponibile",
        "Best deal" => "Offerta migliore",
        "Choose payment method" => "Scegli un metodo di pagamento",
        "No payment methods available" => "Nessun metodo di pagamento disponibile",
        "This signs you out of Murglar, SoundCloud, and Deezer on this device." => {
            "Questo ti disconnetterà da Murglar, SoundCloud e Deezer su questo dispositivo."
        }
        "Save a copy of your settings, or restore them from a settings file." => {
            "Salva una copia delle impostazioni oppure ripristinale da un file."
        }
        "Accounts and sign-in details stay on this device and are never included." => {
            "Gli account e i dati di accesso restano su questo dispositivo e non vengono inclusi."
        }
        "JSON settings" => "Impostazioni JSON",
        "Settings imported" => "Impostazioni importate",
        "Could not import settings" => "Impossibile importare le impostazioni",
        "Settings exported" => "Impostazioni esportate",
        "Could not export settings" => "Impossibile esportare le impostazioni",
        "Import" => "Importa",
        "Export" => "Esporta",
        "Open kekkodance on GitHub" => "Apri kekkodance su GitHub",
        "Open Chromium" => "Apri Chromium",
        "Open GPUI" => "Apri GPUI",
        "Open gpui-component" => "Apri gpui-component",
        "Open Steinberg ASIO SDK" => "Apri Steinberg ASIO SDK",
        "Open Font Awesome" => "Apri Font Awesome",
        "Could not open GitHub" => "Impossibile aprire GitHub",
        "Could not open Chromium" => "Impossibile aprire Chromium",
        "Could not open GPUI" => "Impossibile aprire GPUI",
        "Could not open gpui-component" => "Impossibile aprire gpui-component",
        "Could not open ASIO SDK" => "Impossibile aprire ASIO SDK",
        "Could not open Font Awesome" => "Impossibile aprire Font Awesome",
        " cursor resources are licensed under the BSD 3-Clause License." => {
            " le risorse dei cursori sono distribuite con licenza BSD 3-Clause."
        }
        " is developed by Zed Industries." => " è sviluppato da Zed Industries.",
        " is licensed under Apache 2.0." => " è distribuito con licenza Apache 2.0.",
        " sources are used under the GPLv3 option." => {
            " i sorgenti sono usati secondo l'opzione GPLv3."
        }
        " Free icons are licensed under CC BY 4.0." => {
            " le icone gratuite sono distribuite con licenza CC BY 4.0."
        }
        "Send playback audio to MiniMeters. Set Audio Source in MiniMeters to Audio Server (Plug-In)." => {
            "Invia l'audio in riproduzione a MiniMeters. In MiniMeters imposta Audio Source su Audio Server (Plug-In)."
        }
        "Control music playback and volume automatically during Counter-Strike 2 matches." => {
            "Controlla automaticamente la riproduzione e il volume durante le partite di Counter-Strike 2."
        }
        "MiniMeters Integration" => "Integrazione MiniMeters",
        "Opening MiniMeters..." => "Apertura di MiniMeters...",
        "Enable MiniMeters Integration before opening its interface" => {
            "Attiva l'integrazione MiniMeters prima di aprire la sua interfaccia"
        }
        "Counter-Strike 2 Integration" => "Integrazione Counter-Strike 2",
        "When the round starts" => "All'inizio del round",
        "When you're dead" => "Quando sei morto",
        "When the round ends" => "Alla fine del round",
        "Disabled" => "Disattivato",
        "Waiting for Counter-Strike 2" => "In attesa di Counter-Strike 2",
        "No active match" => "Nessuna partita in corso",
        "Active round" => "Round attivo",
        "Player dead" => "Giocatore morto",
        "Between rounds" => "Tra un round e l'altro",
        "Transitions" => "Transizioni",
        "Smooth volume changes when gameplay state changes." => {
            "Modifica gradualmente il volume quando cambia lo stato di gioco."
        }
        "Fade in" => "Dissolvenza in entrata",
        "Fade out" => "Dissolvenza in uscita",
        "Repair integration" => "Ripara l'integrazione",
        "Installing..." => "Installazione...",
        "Counter-Strike 2 integration installed" => "Integrazione Counter-Strike 2 installata",
        "Restart Counter-Strike 2 if it is currently running." => {
            "Riavvia Counter-Strike 2 se è già in esecuzione."
        }
        "MiniMeters UI could not be opened" => "Impossibile aprire l'interfaccia di MiniMeters",
        "The MiniMeters host is no longer running" => {
            "Il processo di MiniMeters non è più in esecuzione"
        }
        "The MiniMeters integration is currently available only on Windows and Linux" => {
            "L'integrazione MiniMeters è disponibile solo su Windows e Linux."
        }
        "N/A" => "N/D",
        _ => return None,
    })
}
