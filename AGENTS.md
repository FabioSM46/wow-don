# WoW-don — Tauri 2 (Rust + React)

Gestore di addon per World of Warcraft su Linux, dove il gioco gira dentro un prefisso Wine.
App desktop nativa: backend **Rust** con **Tauri 2**, frontend **React + TypeScript** (Vite,
pnpm). Contesto, decisioni, architettura e roadmap sono in `docs/`.

## Struttura del repository

```
wow-don/
├── AGENTS.md             # Questo file — regole globali (autorevole per la pipeline)
├── CLAUDE.md             # Rimando a questo file per Claude Code
├── README.md             # Panoramica del progetto
├── src/                  # Frontend React + TypeScript
├── src-tauri/            # Backend Rust e configurazione Tauri
├── docs/                 # context, decisions, architecture, roadmap
├── scripts/              # gh-automation.sh, classificatore e gate della CI, test dell'automazione
├── .github/              # CI, review DeepSeek, labeler delle PR, ciclo di vita delle iterazioni
├── .claude/skills/       # Skill Claude canoniche (/dev-issue, /process-pr, /scrum-master)
├── .agents/skills/       # Adapter Codex generati ($dev-issue, $process-pr, $scrum-master)
└── .opencode/skills/     # Adapter OpenCode generati (/dev-issue, /process-pr, /scrum-master)
```

## Aree

| Area       | Cartelle | Tecnologia | Responsabilità |
| ---------- | -------- | ---------- | -------------- |
| `backend`  | `src-tauri/` | Rust (edition 2024), Tauri 2 | Comandi Tauri, configurazione, installazioni di WoW, lettura dei `.toc`, fingerprint, client delle fonti, download ed estrazione, stato locale |
| `frontend` | `src/`, `public/`, `index.html`, `vite.config.ts`, `tsconfig*.json` | React 19, TypeScript, Vite | Liste, ricerca, filtri, ordinamento, chiamate `invoke()` |
| `shared`   | radice | — | Modifiche che toccano entrambe le aree o la configurazione comune (`package.json`, `pnpm-lock.yaml`) |

### Rete, disco e chiavi stanno nel backend — sempre

Il frontend mostra dati e chiama comandi Tauri; non conosce la chiave API e non parla con le
fonti. Tutto quello che tocca rete, file system o segreti è nel backend. Un comando che
restituisce la chiave al frontend, o un `fetch` verso CurseForge dal frontend, è un bug anche
quando "funziona".

### Le regole delle fonti

- La chiave CurseForge resta nel backend: non va al frontend, non finisce nei log, negli errori
  restituiti o nei test.
- Niente scraping del sito CurseForge e niente riuso di chiavi di altre app.
- Gli addon con `allowModDistribution = false` non si scaricano in automatico.
- Gli zip si estraggono in una cartella temporanea e si controlla che nessun percorso esca da
  `Interface/AddOns` prima di spostarli.

### Cambiamenti che toccano entrambe le aree

1. Prima il backend: tipi serializzati (`serde`) e comando Tauri.
2. Poi il frontend, contro il comportamento reale del comando.
3. Esegui i gate di entrambe le aree prima di aprire la PR.

### Interfaccia: minimale e moderna

La UI deve essere **minimale e moderna** (decisione D20 in `docs/decisions.md`). In pratica:

- **Poche superfici.** Una schermata fa una cosa; niente pannelli, barre e finestre accessorie
  che non servono al compito. Le azioni secondarie stanno dietro un menu o compaiono al
  passaggio del mouse, non tutte in vista.
- **Spazio e gerarchia, non decorazione.** Spaziatura generosa e regolare, tipografia pulita
  (font di sistema), al massimo due pesi e pochi corpi. Niente gradienti, ombre pesanti, bordi
  doppi, icone decorative o animazioni che non comunicano uno stato.
- **Colore con parsimonia.** Una palette neutra con un solo colore d'accento; il colore segnala
  stato (errore, avviso, selezione), non abbellisce. Chiaro e scuro sono entrambi obbligatori:
  oggi `src/App.css` cambia solo testo e sfondo con `prefers-color-scheme`; i colori nuovi vanno
  definiti come variabili CSS in `:root`, con i valori del tema scuro nel blocco
  `prefers-color-scheme: dark`, invece di essere scritti nei singoli componenti.
- **I dati in primo piano.** Ricerca, filtri e ordinamento sono il punto di forza dell'app (D4):
  la casella di ricerca è sempre visibile, i filtri sono compatti, le tabelle sono dense ma
  leggibili.
- **Stati sempre gestiti**: caricamento, lista vuota (con una frase che dice cosa fare) ed
  errore, senza dialoghi bloccanti del browser.
- **Nessuna libreria di componenti o di stile** senza decisione esplicita: CSS semplice finché
  basta.

Una PR che tocca `src/` descrive in una riga come rispetta queste regole.

## Confine di privacy: scrivere come se il repository fosse pubblico

Il repository oggi è privato e potrebbe diventare pubblico. Quando succede, tutto quello che è
già stato pubblicato diventa leggibile per sempre: file, metadati dei commit (autore e
committer), nomi dei branch, messaggi di commit e di PR, commenti di review, log della CI,
artifact. Per questo valgono già adesso le regole di un repository pubblico.

- **Mai un indirizzo email reale.** I commit usano l'indirizzo `noreply` fornito da GitHub
  (copialo da Settings → Emails, non scrivere il numero a mano). Codice, fixture e messaggi
  possono contenere solo identità `noreply` di GitHub e indirizzi su domini di esempio riservati.
- **Mai percorsi della propria macchina.** Niente nomi utente, home directory, mount point o
  posizioni dei checkout. La documentazione usa segnaposto come `<repo-root>`, `<worktree>` e
  `<workspace>`; i test usano percorsi chiaramente sintetici.
- **Mai dati personali** nelle issue, nelle PR o nel codice: nomi reali, numeri di telefono,
  posizioni, identificativi di account. L'identità pubblica del progetto è l'handle `@FabioSM46`.
- **Mai segreti o identificativi operativi.** Chiavi API, token, credenziali, hostname interni e
  endpoint non pubblici stanno nei GitHub Secrets o in file locali ignorati (`.env`), mai in git,
  nei log, nelle issue o nelle PR.
- **Mai un riferimento a una sessione di un agente.** Un trailer `Claude-Session:` o un link
  `claude.ai/code/session_…` in un messaggio è un endpoint legato a un account.
- **Mai attribuire un commit a un account che non è il tuo.** Un indirizzo
  `<id>+<login>@users.noreply.github.com` accredita chi possiede `<id>`, non `<login>`: il numero
  sbagliato accanto al nome giusto accredita uno sconosciuto. I tre script di privacy tengono una
  tabella login → id per le identità che il repository pubblica.

**Tre script dividono il lavoro, e condividono le stesse tre categorie** (email non pubblica,
percorso della macchina, indirizzo di rete privato), copiate in ciascuno e tenute uguali da
`scripts/test/commit-privacy.test.sh`:

- `bash scripts/check-publication-privacy.sh` legge il contenuto dei file nell'indice (quello che
  un push pubblica, non la cartella di lavoro); con `<base> <head>` legge anche l'albero di ogni
  commit del range, perché un dato aggiunto in un commit e tolto in quello dopo lascia pulita la
  punta ma resta nella storia pubblicata; con `--all-history` legge ogni commit raggiungibile;
- `bash scripts/check-commit-privacy.sh <base> <head>` legge autore, committer e messaggio di ogni
  commit che il branch aggiunge;
- `bash scripts/check-body-privacy.sh` legge un corpo di issue o PR da stdin. Usalo prima di
  pubblicare: `bash scripts/check-body-privacy.sh < corpo.md`.

Esegui i primi due prima di ogni push, entrambi con il range del branch
(`"origin/$BASE_BRANCH" HEAD`): la CI li esegue nel job `detect` sul range della PR, e una violazione blocca la
PR anche con tutto il resto verde. Le diagnostiche nominano solo posizione e categoria, mai il
valore trovato; quando un controllo fallisce, togli il dato prima di continuare invece di
ristamparlo in un altro messaggio.

**Il corpo di una issue o di una PR si può controllare solo dopo.** `.github/workflows/body-privacy.yml`
gira su `issues` e `pull_request_target` (`opened`, `edited`): se trova qualcosa aggiunge la label
`needs-privacy-review` e un commento con categoria e riga; non chiude, non modifica e non
nasconde niente. **Non può impedire una fuga, può solo accorciare il tempo in cui nessuno lo sa**:
un run verde significa "niente è pubblicato ora", mai "niente è mai stato pubblicato". È la
superficie che una macchina scrive più spesso: un agente che scrive il corpo di una PR trascrive
quello che ha appena fatto, e l'ha fatto in una cartella con un nome.

**Prima di rendere pubblico il repository**: esegui `check-publication-privacy.sh --all-history`
e `check-commit-privacy.sh` su tutta la storia. Gli script riconoscono solo le tre categorie
(email, percorsi, indirizzi di rete privati) e i file `.env`: non sono uno scanner di segreti.
Chiavi API e token con altre forme non li vedono, quindi passa anche uno scanner dedicato
(per esempio gitleaks) su tutta la storia. Poi rileggi a mano titoli e corpi di issue e PR già
aperte, controlla i secret e verifica che i ruleset siano ancora attivi dopo il cambio di
visibilità.

## Convenzioni comuni

- **Niente segreti nel codice**: chiavi, token e credenziali stanno nei GitHub Secrets o in file
  locali ignorati, mai in git, nei log, nelle issue o nelle PR.
- **Commit**: mai committare senza richiesta esplicita; stile conventional commit (`feat:`,
  `fix:`, `refactor:`, `docs:`, `ci:`).
- **Prima leggere, poi scrivere**: cerca i pattern esistenti prima di implementare.
- **Nessuna nuova dipendenza** (crate o pacchetti npm) senza istruzione esplicita.
- **Mai leggere i file `.env`**: gli agenti AI NON devono leggere, aprire o ispezionare nessun
  file `.env`. Per sapere quali variabili servono si usa solo `.env.example`. `.env` contiene la
  chiave API reale.
- **Nessuna firma degli strumenti**: niente trailer `Co-Authored-By` di agenti, niente
  "Generated with ..." nelle PR, niente link `claude.ai/code/session_…` nei messaggi.
- **MAI push o merge su `main`**: `main` è il branch di rilascio e resta solo umano. L'AI non
  deve mai fare `git push origin main` né unire una PR in `main`. Le PR verso `main` sono
  ammesse; il merge lo fa l'umano.
- **Il merge verso basi diverse da `main` spetta all'AI**, a suo giudizio. `READY TO MERGE` è un
  segnale da leggere, non una precondizione. Il merge si fa con
  **`bash scripts/gh-automation.sh pr-merge <pr>`**, che rifiuta una base `main` per nome e
  fallisce chiuso su una base illeggibile. Vedi "Cosa ferma davvero un merge" più sotto.
- **Isolamento con git worktree**: ogni modifica e ogni operazione git avviene in un
  `git worktree` isolato, mai nella cartella principale. Usa
  `<cartella-padre>/wow-don-issue-<numero>` quando il lavoro è legato a una issue; se non c'è una
  issue non inventare un numero, usa `<cartella-padre>/wow-don-issue-<slug-descrittivo>`. I
  worktree di `/process-pr` sono `wow-don-pr-<numero>`. Verifica di essere nel worktree prima di
  modificare (`git rev-parse --show-toplevel`). Dopo l'apertura della PR rimuovilo con
  `git worktree remove` e `git worktree prune`. Riusa un worktree esistente per lo stesso branch.

## Strategia dei branch

- `main` — branch di rilascio
- `develop` — branch di integrazione e base di default (branch predefinito del repository); i
  sotto-branch impilati possono partire dal loro branch di feature
- `feature/<issue>-<slug>` — funzionalità e miglioramenti
- `fix/<issue>-<slug>` — correzioni
- `refactor/<issue>-<slug>` — ristrutturazioni senza cambi di comportamento

Per lavoro autorizzato senza issue, ometti il numero invece di inventarlo:
`<tipo>/<slug-descrittivo>` (per esempio `refactor/estrai-config`).

## Pipeline di sviluppo guidata dall'AI

Il repository ha una pipeline che porta le issue dalla creazione a PR pronte per il merge, su tre
livelli:

| Livello      | Tecnologia                              | Responsabilità |
| ------------ | --------------------------------------- | -------------- |
| **Skill**    | Skill per Claude, Codex e OpenCode      | Implementazione, gestione delle PR, cerimonie scrum |
| **Workflow** | GitHub Actions (`.github/workflows/`)   | CI, review delle PR, etichettatura, transizioni delle iterazioni |
| **Helper**   | `scripts/gh-automation.sh`              | Wrapper comune per le API REST e GraphQL |

L'intera infrastruttura è portata da voxelheim-v2; i commenti nel codice dei workflow e degli
script citano numeri di issue e PR di quel repository, dove si trova la storia completa.

### Sincronizzazione delle skill tra runtime

`.claude/skills/` è la fonte canonica. Codex usa gli adapter generati in `.agents/skills/`,
OpenCode quelli in `.opencode/skills/`. Quando una skill canonica cambia:

1. Modifica solo `.claude/skills/<skill>/SKILL.md`.
2. Esegui `bash scripts/sync-agent-skills.sh` dalla radice.
3. Esegui `bash scripts/test/agent-skills-sync.test.sh` e committa le tre versioni insieme.

Non modificare mai a mano gli adapter generati. La CI fallisce se un adapter è vecchio.

### Flusso

```
Issue aperta dal template
        │
        ▼
/dev-issue <numero> in Claude/OpenCode, oppure $dev-issue <numero> in Codex
        │
        ▼
skill: legge la issue → valida l'area → legge AGENTS.md e docs/
        │
        ▼
skill: worktree + branch dalla base (develop di default) → implementa → gate (fmt, clippy, build, test)
        │
        ▼
skill: apre la PR verso quella base → ESCE (senza stato)
        │
        ▼
deepseek-pr-review.yml: commenti di review inline sul diff ─────────┐
        │                                                           │
        ▼                                                           ▼ (in parallelo)
pr-labeler.yml (a fine CI/review + giro ogni 6h): legge ci-gate e thread
        │
    ┌───┴──────────────┐
    │ Regola congelata? │──sì──→ label READY TO MERGE
    │ CI rossa?         │──sì──→ label needs-work
    │ altrimenti        │──────→ needs-review
    └───────────────────┘
        │
        ▼
/process-pr (Claude/OpenCode) o $process-pr (Codex): conflitti, CI, feedback
        │
        ▼
L'AI legge il risultato e i vincoli d'ordine → merge verso basi diverse da main
```

### Skill

| Skill          | Claude / OpenCode         | Codex                     | Comportamento |
| -------------- | ------------------------- | ------------------------- | ------------- |
| `dev-issue`    | `/dev-issue <numero>`     | `$dev-issue <numero>`     | Senza stato: issue → PR → esce |
| `process-pr`   | `/process-pr <numero-pr>` | `$process-pr <numero-pr>` | Correzione attiva: conflitti, CI, feedback di review |
| `scrum-master` | `/scrum-master <cerimonia>` | `$scrum-master <cerimonia>` | backlog-refine, iteration-plan, feature-spec |

Le skill possono essere avviate da un agente oltre che da una persona: non usare
`disable-model-invocation`. Tieni stretti gli `allowed-tools`: sono il motivo per preferire una
skill a un agente costruito a mano.

Le cerimonie delle iterazioni restano con l'umano nel ciclo: scegliere quali issue entrano nella
prossima iterazione è una decisione dell'utente.

#### Cosa ferma davvero un merge

**Verso `develop`: i ruleset e il giudizio dell'AI.** Il ruleset del branch richiede una PR, i
thread di review risolti e (una volta attivato il secondo ruleset) `ci-gate` verde. La regola
congelata calcola `READY TO MERGE`; è un segnale, non un blocco.

**Verso un altro branch di feature: il controllo fresco di chi fa il merge.** I ruleset coprono
solo `develop` e `main`. Prima del merge esegui `is-ready-to-merge` subito prima di `pr-merge`,
e passa `--head <sha-osservato>` e `--base-head <sha-base-osservato>` così che una testa o una
base mosse invalidino la lettura.

**Verso `main`: l'istruzione, più un controllo sul percorso designato.** Un deny può bloccare
`git push origin main` perché il comando nomina il ref. **`gh pr merge <n>` non nomina nessun
branch**: la base è una proprietà della PR, quindi nessun prefisso `Bash(...)` distingue un merge
verso `develop` da uno verso `main`. Al suo posto c'è `pr-merge`, che legge `baseRefName` e
rifiuta `main`, fallendo chiuso su una base vuota o illeggibile senza tentare il merge
(`scripts/test/pr-merge-guard.test.sh`).

**Ferma un incidente, non una decisione.** `gh pr merge` e
`gh api -X PUT repos/OWNER/REPO/pulls/N/merge` aggirano l'helper, e niente ferma un agente che
vuole usarli. L'allowlist non è una sandbox: le skill hanno `Bash(gh *)` e `Bash(bash *)`, ed
entrambi sono necessari. Quello che tiene `main` è l'istruzione.

Le voci `deny` in `.claude/settings.json` per `git push origin main` (e le varianti `--force`,
`-f`, `+main`) restano come comodità: sotto c'è un blocco completo lato remoto.

**Rischio residuo.** Niente legge una PR prima che arrivi su `develop` se non lo fa l'AI.
`unresolved_threads == 0` non significa che una persona abbia guardato: l'AI può risolvere un
thread a cui ha risposto, anche respingendo il suggerimento, e quella risposta è l'intera traccia
di audit. L'ordine tra PR è invisibile alla regola congelata: un ordine scritto nel corpo di una
PR vincola chi fa il merge. **Leggi i corpi.**

### Regola congelata di accettazione

**La label `READY TO MERGE` si aggiunge solo quando: il check stabile `ci-gate` è presente e
verde sul commit di testa, nessun check è fallito o in corso, la PR è mergiabile, nessuna review
richiede modifiche, i thread di review non risolti sono zero, nessuna review DeepSeek ha finding
non letti nel corpo (si chiudono con la label `DEEPSEEK_REVIEW_READ`) e la review DeepSeek è
definitivamente finita (verdetto pulito, round esauriti o esenzione `NO_DEEPSEEK_REVIEW`).**

La regola ha una sola implementazione, `cmd_pr_status_json` in `scripts/gh-automation.sh`, usata
da `pr-labeler.yml` e da `pr-status`. Ogni condizione fallisce chiusa: un conteggio illeggibile è
`-1`, mai `0`.

"Verde" non è "non rosso". Una PR **senza nessun check** soddisfa "zero falliti, zero in corso",
ed è uno stato raggiungibile: una PR in conflitto non ha un merge ref e quindi non esegue nessun
workflow `pull_request`. Per questo l'helper richiede `ci-gate` **presente e verde** e
`mergeable == MERGEABLE`.

Il labeler gira su `workflow_run` quando finiscono la CI o la review DeepSeek, più un giro ogni
sei ore per le transizioni senza evento (risoluzione dell'ultimo thread, mergeabilità ricalcolata
dopo un merge sulla base). `workflow_dispatch` o `/process-pr` forzano un passaggio. Non
aggiungere un trigger `pull_request` al labeler.

**Una scrittura di label fallita fa fallire il labeler.** `pr-label` esce non zero se la
scrittura non arriva, e `pr-check-label` risponde **0 presente / 1 assente / 2 non
determinabile**. Un run rosso del labeler significa che una scrittura è fallita: leggi il log.

#### Il conteggio dei thread vede solo metà della review

DeepSeek consegna i finding in due forme. I commenti inline creano **thread**, che la regola
conta. I commenti generali stanno nel **corpo** della review e non creano thread: una review
fatta solo di quelli conta `0 non risolti` pur avendo finding reali. Per questo `pr-status-json`
conta anche `deepseek_unread_findings`. Il verdetto pulito si riconosce dal marcatore
`<!-- deepseek:no-findings -->` all'inizio del corpo, senza `<!-- deepseek:full-review -->`.

**Il riconoscimento è la label `DEEPSEEK_REVIEW_READ`, ed è datata**: contano come lette solo le
review inviate prima che la label fosse applicata. Una seconda review forzata blocca di nuovo;
togliere la label annulla il riconoscimento. Un umano la può applicare dopo aver letto i finding.
L'AI solo dopo aver letto **ogni** finding, corretto quelli validi o respinto gli altri con prove,
e pubblicato le risposte sulla PR; la sua scrittura deve essere più recente delle review che
riconosce. `NO_DEEPSEEK_REVIEW` resta solo umana.

### Ciclo di vita delle iterazioni guidato dal completamento

Non c'è un calendario e le milestone non hanno scadenza. `iteration-lifecycle.yml` gira quando
si chiude una issue con milestone o con label `ceremony`, individua la milestone attiva e la
valuta:

1. Finché l'iterazione attiva ha issue aperte, non fa niente.
2. Quando si chiude l'ultima issue, crea una cerimonia di backlog refinement per quella
   milestone.
3. La chiusura di quella cerimonia crea una cerimonia di iteration planning.
4. `/scrum-master iteration-plan` sceglie un lotto di lavoro, crea e popola la milestone
   `Iteration N` successiva, poi chiude quella completata.

**Possono esserci più iterazioni aperte**: l'attiva è la milestone aperta con la sequenza **più
bassa** (da `Iteration N` / `Sprint N`, altrimenti dal numero della milestone), confrontata come
numero. Due milestone aperte con la stessa sequenza più bassa fanno fallire il run.

**Avvio a freddo**: senza nessuna milestone, `iteration-advance` non fa niente. Il primo
`/scrum-master iteration-plan` crea direttamente `Iteration 1`.

### Review automatica (DeepSeek)

`deepseek-pr-review.yml` fa la review del codice con l'API DeepSeek (`deepseek-flash`,
`reasoning_effort=high`):

| Modo | Trigger | Comportamento |
|------|---------|---------------|
| **A — Review completa** | PR aperta o nuovo commit | Legge tutto il diff e pubblica commenti inline con suggerimenti. Un finding che nomina un file va ancorato a quel file; il corpo è riservato alle osservazioni che non appartengono a un singolo file. |
| **B — Risposta** | Risposta a un commento di DeepSeek | Legge diff e thread, risponde nello stesso thread. Non tocca mai il codice. |

**Nessuna approvazione**: GitHub non permette ad Actions di approvare PR. Il verdetto pulito è
una review COMMENT che inizia con `<!-- deepseek:no-findings -->`.

**Limite di round**: le review complete automatiche sono al massimo **1** per PR (`MAX_ROUNDS`).
Dopo, i push non fanno partire altre review e viene pubblicato un avviso una volta sola. Le
risposte (Modo B) continuano senza limite. Per forzare un altro passaggio:
`bash scripts/gh-automation.sh pr-deepseek-force-review <pr> [ref]` (`ref` di default `develop`).
Con `--measure-only` rifà la review di un diff senza pubblicare niente.

**Quale copia della configurazione vale**: i run `pull_request` usano il merge ref, quindi
eseguono workflow e script del **branch base**.

**Limite del diff: 90,000 caratteri** (`DEEPSEEK_MAX_DIFF_CHARS`, misurato in voxelheim-v2 con
diciassette repliche a `high`). Oltre, il diff viene troncato, ogni file non letto viene nominato
nel log **e nella review come finding**, e la PR resta bloccata finché qualcuno non lo riconosce.
Una review troncata non può tornare pulita. Dividere la PR è la soluzione affidabile; meglio
ancora dimensionare la issue prima (vedi `/scrum-master` e `/dev-issue` Passo 5). Il limite e
l'effort sono una sola misura: cambiare uno dei due fa fallire `deepseek-budget.test.sh` finché
il limite non viene rimisurato.

Codice generato (cartelle `gen/`, `*_generated.*`) e lockfile (`Cargo.lock`, `pnpm-lock.yaml`,
`go.sum`, per nome esatto) sono esclusi dal diff. I manifest (`Cargo.toml`, `package.json`)
restano: una nuova dipendenza è proprio quello che un reviewer deve vedere.

**Budget e tempi**: `DEEPSEEK_MAX_OUTPUT_TOKENS=384000`; il budget di richiesta è
`DEEPSEEK_REQUEST_TIMEOUT_SECONDS` × (`DEEPSEEK_MAX_RETRIES` + 1) = 2700s × 2 = 90 minuti, e il
job ha `timeout-minutes: 100`. **Il budget deve restare sotto il limite del job**:
`scripts/test/deepseek-budget.test.sh` lo verifica. Se vince il limite del job, lo step risulta
`cancelled` senza output e `pr-status-json` conta `CANCELLED` come fallito.

**Un diff illeggibile fa fallire il run** (errore di fetch, file senza patch, errori di rete), e
un conteggio dei round illeggibile non vale zero.

**Sicurezza**: DeepSeek non crea commit, non fa push e non modifica file. Ignora i propri
commenti. Non dichiara mai di aver fatto azioni che il suo script non può fare (come risolvere un
thread).

### Prerequisiti

0. **Strumenti locali**: `gh` (2.18.0 o successivo) e `jq`, vedi la tabella in `README.md`.
   `scripts/gh-automation.sh` li richiede entrambi.
1. **Token GitHub**: un PAT fine-grained con `contents: read/write`, `pull_requests: read/write`,
   `issues: read/write` su questo repository, salvato come secret `GH_PIPELINE_TOKEN`. Serve al
   labeler e al ciclo di vita delle iterazioni (le scritture fatte con `GITHUB_TOKEN` non
   attivano altri workflow).

   **Questo token non legge lo stato della CI, e nessun permesso lo risolve**: la Checks API è
   solo per GitHub App. `pr-labeler.yml` passa `GH_CI_TOKEN: ${{ secrets.GITHUB_TOKEN }}` con
   `checks: read`, `statuses: read` e `actions: read`, e `gh_ci()` instrada lì quella sola
   lettura. **Ogni nuovo chiamante che legge i check deve usare `gh_ci`, non `gh`.**
2. **Chiave API DeepSeek**: secret `DEEPSEEK_API_KEY` (`Settings → Secrets and variables →
   Actions`).
3. **Protezione dei merge**: due ruleset su `develop` e `main`, vedi
   `.github/branch-protection.md`. Il secondo (`ci-gate` obbligatorio) si attiva solo dopo che
   `ci-gate` ha girato su una PR vera.
4. **Label**: `READY TO MERGE`, `needs-review`, `needs-work`, `DEEPSEEK_REVIEW_READ`,
   `NO_DEEPSEEK_REVIEW`, `ceremony`, `feature`, `bug`, `needs-refinement`, `needs-triage`,
   `ready-for-dev`, `needs-privacy-review`.

## Definition of Done

**Ogni PR aperta dalla pipeline DEVE verificare TUTTO questo prima di essere creata:**

- [ ] **Formattazione** — `cargo fmt --all --check` (in `src-tauri/`)
- [ ] **Lint** — `cargo clippy --all-targets --locked -- -D warnings`
- [ ] **Build** — `cargo build --locked` e `pnpm build` (`tsc && vite build`)
- [ ] **Test** — `cargo test --locked`
- [ ] **Automazione** — `scripts/test/*.test.sh` e `python3 .github/scripts/test_deepseek_review.py`
- [ ] **Nessuna stampa di debug** nei percorsi di produzione
- [ ] **Nessun segreto** — niente chiavi, token, password o credenziali nel codice o nei test
- [ ] **Chiave API solo nel backend** — mai nel frontend né nei log
- [ ] **Privacy di pubblicazione pulita** — nessuna email reale, dato personale, percorso della
      macchina, hostname o endpoint privato; `scripts/check-publication-privacy.sh` passa
      sull'albero e `scripts/check-commit-privacy.sh` sui commit che il branch aggiunge

Se un gate fallisce, l'implementazione non è finita.

### Cosa impone la CI

`.github/workflows/ci.yml` esegue:

| Job          | Gate |
| ------------ | ---- |
| `detect`     | Controlli di privacy sull'albero e sui commit della PR, poi classifica i file cambiati con `scripts/changed-areas.sh` |
| `backend`    | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build --locked`, `cargo test --locked` (con le librerie di sistema di Tauri) |
| `frontend`   | `pnpm install --frozen-lockfile`, `pnpm build` |
| `automation` | Tutta la suite `scripts/test/*.test.sh` più `test_deepseek_review.py` |
| `ci-gate`    | L'unico check stabile: verifica che ogni job selezionato abbia avuto successo e ogni salto fosse autorizzato |

Su una PR verso una base diversa da `main`, `detect` seleziona i job dal diff: `src-tauri/**`
attiva `backend`, `src/**` e i file di toolchain del frontend attivano `frontend`,
`package.json`/`pnpm-lock.yaml` attivano entrambi, `ci.yml` attiva tutto, la documentazione non
attiva niente. `automation` gira sempre. Un percorso non riconosciuto o un diff illeggibile
attivano tutto (fallimento aperto). Su una PR verso `main` gira tutto.

Due trappole:

1. **Mai filtri `paths:` a livello di workflow o di job.** Un workflow filtrato non produce un
   risultato che `ci-gate` possa verificare. Lo strumento giusto è `if:` a livello di job.
2. **`cancel-in-progress` produce job CANCELLED, che `pr-status-json` conta come falliti.** In
   `ci.yml` la cancellazione vale solo per eventi `pull_request`, dove la run cancellata
   appartiene sempre a un commit che non è più la testa.

Le action sono fissate per SHA completo con l'etichetta della release, e il checkout non
conserva le credenziali (`scripts/test/actions-hardening.test.sh`). Il toolchain Rust è fissato
in `src-tauri/rust-toolchain.toml`, allineato a `dtolnay/rust-toolchain` in CI.
