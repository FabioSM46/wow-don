---
name: scrum-master
description: Scrum ceremony facilitator. Use for backlog refinement, iteration planning, and feature specification.
argument-hint: <ceremony>
allowed-tools: Bash(gh *) Bash(git *) Bash(cat *) Bash(xargs *) Bash(date *) Bash(echo *) Bash(set *) Bash(jq *)
---

# scrum-master — cerimonie scrum

Trigger: `/scrum-master <cerimonia>`, dove la cerimonia è una di:
- `backlog-refine` — raffinamento del backlog
- `iteration-plan` — pianificazione di un'iterazione guidata dal completamento
- `feature-spec` — funzionalità → specifica tecnica → issue

## Scopo

Gestisce il ciclo di sviluppo del repository. L'utente porta le idee; questa skill ascolta,
scrive le specifiche tecniche, crea le issue e organizza backlog e iterazioni.

## Cerimonie

### 1. backlog-refine

Trigger: `/scrum-master backlog-refine` oppure da una issue di cerimonia.

**Workflow:**

1. Leggi tutte le issue aperte:
   ```bash
   gh issue list --state open --json number,title,labels,createdAt,updatedAt --limit 100
   ```

2. Escludi le issue di cerimonia (label `ceremony`).

3. Classifica ogni issue rispetto allo stesso contratto di campi obbligatori che `/dev-issue`
   applica (Area, Tipo, Priorità, User story, Criteri di accettazione, Contesto tecnico, Fuori
   scope), leggendo **solo il corpo della issue**: è tutto quello che `/dev-issue` vede.
   - **Pronta**: tutti e sette i campi presenti e sostanziali nel corpo
   - **Da specificare**: un campo obbligatorio manca o è solo un segnaposto
   - **Da smistare**: nuova o non classificata
   - **Ferma**: nessuna attività da 30+ giorni → proporre chiusura o ridefinizione

4. Per ogni issue **Da specificare**:
   - leggi il corpo;
   - scrivi i criteri di accettazione mancanti partendo dalla user story;
   - scrivi il contesto tecnico mancante partendo dall'area e dal codice esistente;
   - scrivi **Fuori scope** se manca, con file/moduli/comportamenti concreti, mai "modifiche non
     correlate": è l'unica difesa di `/dev-issue` contro l'allargamento dello scope;
   - **scrivi ogni campo nel corpo della issue. Un commento non basta.** `/dev-issue` legge la
     issue con `gh issue view <numero> --json title,body,labels` e ricava i campi solo dal corpo,
     non legge mai i commenti.

     `gh issue edit --body-file` sostituisce **tutto** il corpo, quindi la modifica deve essere
     chirurgica per disciplina: sostituisci solo la sezione `### <Campo>` e riproduci ogni altro
     byte. Leggi, riscrivi, sostituisci, poi rileggi il corpo e verifica che l'unica differenza
     sia quella sezione (GitHub aggiunge un solo a capo finale).

     ```bash
     gh issue view <numero> --json body --jq '.body' > /tmp/issue-<numero>.md
     # Riscrivi il file con la sezione sostituita — un heredoc
     # `cat > /tmp/issue-<numero>.md <<'BODY' … BODY`, perché gli allowed-tools di questa
     # skill includono `cat` e nessuno strumento di modifica dei file — poi:
     gh issue edit <numero> --body-file /tmp/issue-<numero>.md
     gh issue view <numero> --json body --jq '.body'   # rileggi e confronta
     ```

   - Aggiungi anche un commento quando una scoperta deve arrivare a una persona tramite le
     notifiche (un rischio d'ordine, una dipendenza tra issue). Il commento è la traccia, la
     modifica del corpo è la consegna. Mai solo il commento.

5. Riordina le priorità in base a:
   - catene di dipendenze (modulo backend prima del comando Tauri, comando prima della UI);
   - valore (la gestione degli addon installati prima del catalogo, il catalogo prima delle
     rifiniture);
   - impegno (alterna vittorie rapide e lavori grandi).

6. Aggiorna le label quando una issue è pronta: `needs-refinement` → `ready-for-dev` solo quando
   tutti e sette i campi sono presenti e sostanziali **nel corpo**. `iteration-plan` sceglie il
   lavoro da `ready-for-dev`, quindi una label guadagnata con un commento pianifica lavoro su cui
   `/dev-issue` si fermerebbe al primo passo.

7. Pubblica un commento di sintesi sulla issue di cerimonia (o in una nuova issue se avviata a
   mano) con:
   - totale delle issue aperte;
   - ripartizione per categoria;
   - ordine di priorità suggerito per la prossima iterazione;
   - issue proposte per la chiusura.

8. Chiudi la issue di cerimonia.

### 2. iteration-plan

Trigger: `/scrum-master iteration-plan` dalla issue di cerimonia creata da
`iteration-lifecycle.yml` per la milestone.

**Avvio a freddo (repository nuovo)**: se NON esiste nessuna milestone di iterazione, aperta o
chiusa, non c'è una milestone da verificare né una cerimonia da aspettare. Salta i passi 1–4:
crea direttamente la milestone senza data `Iteration 1`, popolala da `ready-for-dev` (passi
5–8) e pubblica il piano. Dopo la prima iterazione tutto passa per la macchina a stati guidata
dal completamento.

**Workflow:**

1. Individua l'iterazione attiva: la milestone aperta con la sequenza **più bassa**, letta da un
   titolo `Iteration N` / `Sprint N` e in mancanza dal numero della milestone, la stessa regola di
   `iteration-advance`. Fallisci chiuso se nessuna milestone è aperta (fuori dall'avvio a freddo)
   o se due milestone aperte hanno la stessa sequenza **più bassa**. Una collisione più in alto
   non conta finché non diventa la più bassa.
   ```bash
   REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
   gh api "repos/$REPO/milestones?state=open&per_page=100"
   ```

   **Più milestone aperte sono uno stato previsto.** Pianificare in anticipo è il punto: un
   blocco coerente di lavoro può essere assegnato a un'iterazione futura mentre quella corrente
   è ancora in corso. Creare e popolare un'iterazione è sempre possibile; **chiudere** una
   milestone completata è quello che i passi 2 e 3 controllano.

2. **Per chiudere la milestone completata deve avere zero issue aperte.** Le iterazioni sono
   guidate dal completamento: mai spostare lavoro non finito in automatico e mai chiudere una
   milestone che ha ancora issue aperte. Vale solo per il passo 9.

3. **Per chiuderla serve anche il suo raffinamento.** Leggi il marcatore
   `<!-- iteration-lifecycle:iteration-plan:milestone-N -->` della cerimonia di pianificazione e
   richiedi esattamente una issue di backlog-refinement chiusa con il marcatore corrispondente
   `<!-- iteration-lifecycle:backlog-refine:milestone-N -->`. Rifiuta di chiudere se il
   raffinamento manca, è aperto o è duplicato.

4. Individua la sequenza obiettivo: **quella nel titolo della cerimonia, una sopra l'iterazione
   attiva.** Con la pianificazione anticipata può esistere già:

   - **esiste e contiene lavoro**: il lotto è stato scelto quando è stata pianificata. I passi
     5–7 non hanno niente da selezionare e il passo 8 non crea niente; resta il passo 9.
   - **non esiste, o esiste vuota**: è il caso normale, seleziona il lavoro e creala al passo 8.

   **Mai creare una seconda milestone su una sequenza che ne ha già una.** Per pianificare oltre,
   prendi la sequenza sopra la **più alta** esistente, aperta o chiusa. Non modificare ancora lo
   stato delle milestone.

5. Leggi le issue in ordine di priorità dal raffinamento completato o dalle label di priorità.

6. Seleziona le issue per l'iterazione:
   - un lotto realistico per un solo sviluppatore;
   - bilanciato tra backend e frontend quando il backlog lo permette;
   - almeno un bug o un debito tecnico se ce ne sono;
   - rispetta ogni dipendenza bloccante;
   - almeno una issue selezionata; se nessuna è pronta, spiega il blocco e lascia aperte la
     cerimonia e la milestone completata. Vale solo se il passo 4 ha individuato una milestone
     da creare o a cui aggiungere lavoro.

7. Per ogni issue selezionata:
   - stima: S (ore), M (1–2 giorni), L (3–5 giorni);
   - controlla le dipendenze bloccanti;
   - suggerisci un ordine di implementazione.

8. Crea la milestone senza data per la sequenza del passo 4 (o `Iteration 1` all'avvio a freddo),
   assegnale ogni issue selezionata e verifica le assegnazioni prima di chiudere la milestone
   completata. Quest'ordine lascia la milestone precedente recuperabile se la creazione o
   l'assegnazione falliscono. Se la milestone obiettivo esiste già, assegna lì il lavoro.
   ```bash
   for issue in <numeri-issue>; do
     gh issue edit "$issue" --milestone "Iteration <numero>"
   done
   ```

9. Chiudi la milestone completata solo dopo che la nuova iterazione contiene il lavoro scelto, e
   solo se i passi 2 e 3 sono soddisfatti. Se l'iterazione attiva ha ancora issue aperte,
   lasciala aperta. (Avvio a freddo: niente da chiudere.)

10. Pubblica il piano come commento con:
    - obiettivo dell'iterazione (una frase);
    - elenco delle issue con stima e area;
    - impegno totale stimato;
    - dipendenze e rischi.

11. Chiudi la issue di cerimonia. La chiusura rilancia l'helper del ciclo di vita, che dovrebbe
    vedere la nuova iterazione attiva e non cambiare niente.

### 3. feature-spec

Trigger: `/scrum-master feature-spec <descrizione-funzionalità>`

**Workflow:**

1. Confronta la descrizione con i documenti del progetto: `docs/context.md` (perché esiste
   WoW-don e cosa deve fare meglio di WowUp-CF), `docs/roadmap.md` (milestone),
   `docs/architecture.md` (moduli previsti) e `docs/decisions.md` (decisioni prese e aperte).
   Fai domande se:
   - l'area non è indicata e non è ovvia;
   - lo scope è ambiguo;
   - le dipendenze non sono chiare;
   - la funzionalità contraddice una decisione registrata (di' quale, e chiedi).

2. Scrivi una specifica tecnica (in conversazione):
   - **Sintesi**: un paragrafo sulla funzionalità
   - **Aree coinvolte**: backend, frontend o entrambe
   - **Backend (Rust/Tauri)**: moduli in `src-tauri/src/`, comandi Tauri esposti, tipi
     serializzati verso il frontend, I/O su disco, gestione degli errori
   - **Frontend (React/TypeScript)**: componenti, stato, ricerca/filtri/ordinamento, chiamate
     `invoke()`
   - **Fonti esterne** (se servono): endpoint CurseForge usati, limiti, campi da verificare sulla
     documentazione ufficiale
   - **Dati e persistenza** (se servono): stato locale, formato, migrazioni
   - **Vincoli**: chiave API solo nel backend e mai nei log; niente scraping e niente chiavi di
     altre app; niente download automatico con `allowModDistribution = false`; estrazione degli
     zip sicura (nessun percorso fuori da `AddOns`); percorsi dentro prefissi Wine con spazi;
     nessuna nuova dipendenza senza discuterne
   - **Rischi**: cosa può andare storto, cosa richiede attenzione

3. Fatti approvare la specifica. NON creare issue finché l'utente non conferma.

4. Dopo l'approvazione crea le issue con il formato del template delle funzionalità.

   **Il corpo della issue è un contratto con `/dev-issue`.** Quella skill legge questi titoli e
   considera obbligatori Area, Tipo, Priorità, User story, Criteri di accettazione, Contesto
   tecnico e Fuori scope, come `.github/ISSUE_TEMPLATE/feature_request.yml`. Una issue senza uno
   di questi blocca la pipeline al primo passo. Scrivili **tutti e sette**, sempre.

   I valori dei menu sono stringhe fisse; usali alla lettera:

   | Campo | Valori ammessi |
   |-------|----------------|
   | Area | `backend (Rust / Tauri)` · `frontend (React / TypeScript)` · `shared (Trasversale)` |
   | Tipo | `feature (Nuova funzionalità)` · `enhancement (Miglioramento di una funzionalità esistente)` · `refactor (Ristrutturazione senza cambi di comportamento)` |
   | Priorità | `high (Blocca altro lavoro)` · `medium (Priorità normale)` · `low (Utile ma non urgente)` |

   Per ogni unità di lavoro:

   ```bash
   gh issue create \
     --title "[Feature]: <descrizione del componente>" \
     --body "$(cat <<'EOF'
   ### Area

   <uno dei tre valori ammessi, alla lettera>

   ### Tipo

   <uno dei tre valori ammessi, alla lettera>

   ### Priorità

   <uno dei tre valori ammessi, alla lettera>

   ### User story

   Come <ruolo>, voglio <obiettivo>, così che <motivo>

   ### Criteri di accettazione

   - [ ] <criterio 1>
   - [ ] <criterio 2>

   ### Contesto tecnico

   <file, pattern e vincoli rilevanti dalla specifica>

   ### Fuori scope

   - <cosa questa issue NON deve toccare>

   ### Riferimenti nel codice

   - `src-tauri/src/commands.rs:12` — <perché conta>

   ### Dipendenze

   <issue bloccanti, oppure "Nessuna">

   ### Strategia di test

   <cosa testare e a che livello>
   EOF
   )" \
     --label "feature" \
     --label "needs-refinement"
   ```

   **Fuori scope è il campo con più leva.** È l'unico confine che `/dev-issue` ha contro
   l'allargamento dello scope, ed è quello lasciato vago più spesso. Nomina file, moduli o
   comportamenti concreti.

   I bug usano `.github/ISSUE_TEMPLATE/bug_report.yml`, che richiede Area, Cosa è successo?,
   Cosa doveva succedere?, Passi per riprodurre, Contesto tecnico e Fuori scope, senza
   Tipo/Priorità/User story.

5. Raggruppa le issue:
   - **prima le fondamenta**: tipi e moduli del backend, formati dei dati, client delle fonti;
   - **poi il nucleo**: comandi Tauri e flussi principali della UI;
   - **infine le rifiniture**: casi limite, gestione degli errori, feedback visivo.

6. Riporta all'utente:
   - numero di issue create;
   - ordine di implementazione suggerito;
   - impegno totale stimato;
   - link alla specifica.

## Dimensionamento: una issue si scrive contro il limite della review

`DEEPSEEK_MAX_DIFF_CHARS` è **90,000** caratteri. Una PR oltre il limite viene troncata, ogni
file non letto diventa un finding e la PR resta bloccata finché qualcuno non lo riconosce.
`/dev-issue` Passo 5 divide una issue troppo grande in parti, ma è un rimedio: quando
`/dev-issue` legge la issue, la sua forma è già fissata. Questo è il momento in cui un confine
si sceglie senza codice già scritto.

### Stima ogni issue che scrivi o raffini

Conta, invece di indovinare:

- le **aree** (backend, frontend);
- i **tipi di superficie** che la issue nomina: un meccanismo, un modello dati, un comando
  Tauri, una schermata o un controllo UI, uno stato persistente, un client verso una fonte
  esterna. Un tipo è una PR; due tipi sono di norma due PR;
- i **criteri di accettazione**: oltre sei o sette, i gruppi in cui cadono sono i confini;
- i moltiplicatori: i test valgono circa due terzi di un cambiamento con copertura vera.

Due sintomi che le parti sono più di una, leggibili mentre scrivi il corpo:

- **la sintesi ha bisogno di una "e" tra due nomi di tipo diverso** (un parser *e* una
  schermata, un client *e* la UI che lo usa);
- **più di sei o sette criteri di accettazione.**

### Cosa fare quando una issue è troppo grande

**Meglio dividere la issue.** Due issue, ciascuna una PR, ciascuna con i suoi criteri e la sua
riga di `Dipendenze`, sono meglio di una issue e una nota: la milestone conta il lavoro reale e
`/dev-issue` lavora senza dover decidere da solo.

**Quando il lavoro è davvero una issue sola**, tienila intera e scrivi il confine nel corpo
sotto un titolo `Divisione suggerita`, con ogni parte, cosa contiene e l'ordine di merge.

**Mai dividere su un conteggio di caratteri.** Il confine è uno che il codice traccia già.

### Scrivi gli assi della stima, non il verdetto

Metti la stima e il ragionamento nel corpo (o nel commento di raffinamento): aree contate, tipi
di superficie contati e nominati, moltiplicatori, e una banda dichiarata come **minimo, non come
conteggio**. Non scrivere "Una PR" come verdetto: un agente che legge un numero già scritto lo
eredita invece di rifare la stima, e la decisione la prende chi è più lontano dal codice.

## Guardrail

- Mai creare issue senza conferma della specifica da parte dell'utente.
- Mai modificare issue o milestone chiuse.
- Rispetta le aree: niente issue backend per una funzionalità solo frontend.
- Una funzionalità che porta dati dal backend al frontend ha SEMPRE una parte backend (comando
  Tauri e tipi serializzati): rendila esplicita invece di nasconderla in una issue di UI.
- **Dimensiona ogni issue rispetto al limite di 90,000 caratteri prima di assegnarla a una
  milestone.**
- Nel dubbio sullo scope, chiedi.
- Le issue di cerimonia (label `ceremony`) sono escluse da tutti i report.

## Riferimenti

- Contesto, roadmap, architettura, decisioni: `docs/`
- Template delle issue: `.github/ISSUE_TEMPLATE/`
- Pipeline: `AGENTS.md`
- Limite della review e i suoi costi: `AGENTS.md`, "Review automatica (DeepSeek)"; `/dev-issue` Passo 5
- Helper: `scripts/gh-automation.sh`
