---
name: dev-issue
description: Implements GitHub issues end-to-end. Use to turn an issue into a merge-ready PR.
argument-hint: <issue-number> [--base <branch>]
allowed-tools: Bash(gh *) Bash(git *) Bash(cargo *) Bash(rustup *) Bash(pnpm *) Bash(node *) Bash(python3 *) Bash(bash *) Bash(cd *) Bash(mkdir *) Bash(mktemp *) Bash(trap *) Bash(ls *) Bash(rm *) Bash(cp *) Bash(mv *) Bash(cat *) Bash(xargs *) Bash(sed *) Bash(awk *) Bash(tr *) Bash(head *) Bash(tail *) Bash(paste *) Bash(find *) Bash(rg *) Bash(date *) Bash(dirname *) Bash(echo *) Bash(set *) Bash(source *) Bash(export *) Bash(jq *) Bash(wc *)
---

# dev-issue — dalla issue alla PR

Trigger: `/dev-issue <numero-issue>` oppure `/dev-issue <url-issue>`, con `--base <branch>`
opzionale quando il lavoro va impilato su un branch di feature diverso da `develop`.

## Scopo

Porta una issue GitHub dai requisiti alla PR, senza stato. Esce dopo aver aperto la PR e
ripulito il worktree. **Non** sorveglia la PR: il monitoraggio passivo è di `pr-labeler.yml`
(a eventi più un giro ogni sei ore), la correzione attiva è di `/process-pr`.

Usa `git worktree` per isolare il branch, così più issue possono procedere in parallelo senza
toccare la cartella di lavoro principale.

## Workflow

### Passo 1: leggere la issue

`gh issue view <numero> --json title,body,labels`. Dal corpo si leggono questi campi:

| Campo | Sezione nel corpo | Obbligatorio | Serve per |
|-------|-------------------|--------------|-----------|
| Area | `### Area` → menu | Sì | Cartella di lavoro e gate |
| Tipo | `### Tipo` → menu | Sì* | Prefisso del branch (*i bug non hanno Tipo: si ricava dalla label `bug`) |
| Priorità | `### Priorità` → menu | Sì* | Quanto impegno (*stessa eccezione) |
| User story | `### User story` | Sì* | Obiettivo (*stessa eccezione) |
| Criteri di accettazione | `### Criteri di accettazione` | Sì* | Checklist di verifica (*stessa eccezione) |
| Contesto tecnico | `### Contesto tecnico` | Sì | Guida all'implementazione |
| Fuori scope | `### Fuori scope` | Sì | Confini dell'implementazione |
| Riferimenti nel codice | `### Riferimenti nel codice` | No | File da leggere per primi |
| Dipendenze | `### Dipendenze` | No | Lavoro bloccante |
| Strategia di test | `### Strategia di test` | No | Cosa testare |

L'obbligatorietà rispecchia `.github/ISSUE_TEMPLATE/feature_request.yml`. I bug
(`bug_report.yml`) hanno `Cosa è successo? / Cosa doveva succedere? / Passi per riprodurre` al
posto di Tipo/Priorità/User story/Criteri: i passi per riprodurre sono i criteri di accettazione.

Se manca un campo obbligatorio non indovinare: di' quale manca e chiedi all'utente se procedere.

### Passo 2: validare l'area

| Valore nella issue | Cartella | Gate (Passo 6) |
|--------------------|----------|----------------|
| `backend (Rust / Tauri)` | `src-tauri/` | backend |
| `frontend (React / TypeScript)` | `src/` (più `index.html`, `vite.config.ts`, `tsconfig*.json`, `public/`) | frontend |
| `shared (Trasversale)` | radice | backend e frontend |

**Validazione difensiva**: se il valore non è uno di questi, NON procedere. Chiedi: "Non
riconosco l'area '[valore]'. Valori ammessi: backend (Rust / Tauri), frontend (React /
TypeScript), shared (Trasversale). Quale uso?"

### Passo 3: leggere il contesto

Prima di scrivere codice:

1. Leggi `AGENTS.md` alla radice: convenzioni e contratto della pipeline.
2. Leggi i documenti in `docs/` che la issue tocca: `architecture.md` per i moduli del backend,
   `decisions.md` per le scelte già prese, `roadmap.md` per la milestone.
3. Apri ogni **Riferimento nel codice** della issue.
4. Leggi **Fuori scope** e tienilo a mente: non toccare niente di quell'elenco.
5. Cerca i pattern esistenti con `rg --files` e `rg`.

### Passo 3a: scegliere la base della PR

`BASE_BRANCH=develop`, a meno che l'invocazione passi `--base <branch>`. La base decide sia il
punto di partenza del branch sia il target della PR.

Verifica che il branch remoto esista prima di creare il worktree. Rifiuta una base vuota e
rifiuta `main`: questa skill non apre e non ridirige mai PR verso `main`. Se riusi un branch o
una PR esistenti, verifica che la base reale sia quella richiesta invece di cambiarla in silenzio.

### Passo 4: creare worktree e branch

**Nome del branch:**

```
Tipo (dal menu, o dalle label se manca):
  feature     → feature/
  enhancement → feature/
  refactor    → refactor/
  label bug   → fix/

Slug: titolo → minuscolo → caratteri non alfanumerici in trattini
      → trattini consecutivi compressi → senza trattini iniziali/finali
      → al massimo 5 parole

Risultato: <prefisso><numero-issue>-<slug>

Esempi:
  feature/12-aggiungi-installazione-manuale
  fix/31-toc-duplicati-classic-beta
  refactor/40-estrai-client-curseforge
```

Il percorso del worktree si ricava dalla radice del repo, mai da `$PWD`: un relativo
`../wow-don-issue-N` dopo un `cd src-tauri` creerebbe il worktree *dentro* il repo.

```bash
set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

SLUG=$(echo "<titolo-issue>" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | sed 's/-\+/-/g' | sed 's/^-//;s/-$//' | tr '-' '\n' | head -5 | paste -sd '-')
BRANCH="<prefisso><numero-issue>-${SLUG}"
WORKTREE_DIR="$(dirname "$REPO_ROOT")/wow-don-issue-<numero-issue>"
BASE_BRANCH="<develop-o-base-esplicita>"
[ -n "$BASE_BRANCH" ] && [ "$BASE_BRANCH" != "main" ] || { echo "Refusing PR base: $BASE_BRANCH"; exit 1; }
git fetch origin "$BASE_BRANCH"

# Riusa un worktree già aperto per questo branch.
EXISTING=$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '
  /^worktree /{wt=substr($0,10)}
  /^branch /{if (substr($0,8)==b) {print wt; exit}}')

if [ -n "$EXISTING" ]; then
  echo "Reusing existing worktree: $EXISTING"
  WORKTREE_DIR="$EXISTING"
  cd "$WORKTREE_DIR"
else
  # Tre stati, non due: il branch può esistere senza worktree (il Passo 10 rimuove il
  # worktree e tiene il branch, che contiene i commit della PR). `worktree add -b`
  # fallirebbe con "A branch named '<branch>' already exists".
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    echo "Branch $BRANCH exists with no worktree — checking it out"
    git worktree add "$WORKTREE_DIR" "$BRANCH"
  else
    git worktree add -b "$BRANCH" "$WORKTREE_DIR" "origin/$BASE_BRANCH"
  fi
  cd "$WORKTREE_DIR"
fi

# Dipendenze: node_modules non è condiviso tra worktree.
pnpm install --frozen-lockfile
(cd src-tauri && cargo fetch --locked)
```

Il worktree è `<cartella-padre-del-repo>/wow-don-issue-<numero>`:

- repo principale: `<workspace>/wow-don`
- worktree della issue #12: `<workspace>/wow-don-issue-12`

**Verifica prima di modificare qualunque cosa:**

```bash
[ "$(git rev-parse --show-toplevel)" = "$WORKTREE_DIR" ] || { echo "NOT in worktree — abort"; exit 1; }
```

Da qui in poi ogni modifica e ogni comando git girano dentro `$WORKTREE_DIR`. Se il controllo
fallisce, fermati e rientra nel worktree: non modificare la cartella principale.

Il worktree non ha il file `.env` e non deve averlo: i gate non usano la chiave CurseForge, e
gli agenti non leggono mai `.env`.

### Passo 5: decidere quante PR, poi implementare

**Prima di scrivere codice decidi in quante PR va divisa la issue.** Il Passo 7 misura il diff
finito contro `DEEPSEEK_MAX_DIFF_CHARS` (**90,000** caratteri): oltre quel limite il reviewer
tronca il diff e ogni file non letto diventa un finding che blocca la PR. Dividere dopo aver
scritto tutto è il momento più costoso, perché ogni confine va ritagliato dal lavoro finito.

Stima dalla issue, non a sensazione:

- conta le **aree** toccate (backend, frontend);
- conta i **tipi di superficie** che la issue nomina: un meccanismo, un modello dati, un
  comando Tauri, una schermata o un controllo UI, uno stato persistente, un client verso una
  fonte esterna. Un tipo è una PR; due tipi sono di norma due PR;
- conta i **criteri di accettazione**: oltre sei o sette, i gruppi in cui cadono sono già i
  confini delle parti;
- ricorda che i test pesano: un modulo con test veri vale circa tre volte il codice di
  produzione.

Punta a circa 80,000 caratteri per parte, non a 90,000: il limite è un punto di troncamento,
non un obiettivo. Se il corpo della issue dichiara già un numero di PR, è un minimo da
verificare, non una risposta: rifai la stima e, se non torna, dillo nella descrizione della PR e
segui la tua.

**Il confine è uno che il codice traccia già**: il comando Tauri e la UI che lo usa, il
meccanismo e chi lo chiama, il modello dati e la sua persistenza. Mai un conteggio di caratteri.
Ogni parte deve compilare, passare i gate e reggersi da sola. In un crate binario "da sola" ha
un significato per il compilatore: un elemento `pub` raggiungibile solo dai test è `dead_code`
con `-D warnings`. Sposta il confine oppure usa `#[allow(dead_code)]` con un commento che
nomina la parte che lo userà, e dillo nella PR.

Le parti successive si aprono sul branch della parte precedente (`--base <branch-precedente>`):
la CI gira su PR verso qualunque base. Ogni descrizione dice quale parte è, quante sono, la base
esatta e il verso in cui la pila va chiusa (la foglia più profonda pronta si unisce per prima
nella sua base, poi la base sale).

**Scrivi la stima nella descrizione della PR**, accanto alla misura del Passo 7.

Poi implementa:

1. **Rispetta Fuori scope alla lettera**: se una modifica tocca qualcosa di quell'elenco,
   fermati e rivaluta.
2. Segui i pattern esistenti.
3. **Rete, disco e chiave API stanno nel backend.** Il frontend mostra dati e chiama comandi
   Tauri; non conosce la chiave CurseForge e non fa richieste HTTP verso le fonti.
4. La chiave non va mai nei log, negli errori restituiti al frontend o nei test.
5. Nessuna nuova dipendenza (crate o pacchetti npm) senza istruzione esplicita.
6. Niente segreti nel codice, niente `dbg!`/`println!`/`console.log` di debug lasciati nei
   percorsi di produzione.
7. Modifiche chirurgiche: modifica file esistenti quando puoi, creane di nuovi solo se serve.
8. Gli addon con `allowModDistribution = false` non si scaricano in automatico.
9. Ogni modifica in `src/` segue la sezione "Interfaccia: minimale e moderna" di `AGENTS.md`;
   la PR dice in una riga come la rispetta.

### Passo 6: gate di qualità (OBBLIGATORI)

Questi comandi rispecchiano `.github/workflows/ci.yml`. Quello che salti qui diventa una PR
rossa e un round di review DeepSeek sprecato: il budget è di un round.

Dentro il worktree, per **ogni** area toccata dal diff
(`git diff --name-only "origin/$BASE_BRANCH"...HEAD` più le modifiche non ancora committate):

| Area | Comando |
| ---- | ------- |
| backend (`src-tauri/**`) | `cd src-tauri && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo build --locked && cargo test --locked` |
| frontend (`src/**`, `public/**`, `index.html`, `vite.config.ts`, `tsconfig*.json`) | `pnpm install --frozen-lockfile && pnpm build && pnpm test` |
| `package.json` / `pnpm-lock.yaml` | entrambe le righe sopra |

`pnpm build` è `tsc && vite build`: type-check e bundle insieme. `pnpm test` esegue i
`src/**/*.test.ts` con `node --test` (Node 22.18 o successivo toglie i tipi da solo).

**Il verdetto di un gate è il suo exit status, e quello di una pipeline è dell'ultimo
comando.** `cargo clippy ... 2>&1 | tail -5 && echo OK` stampa `OK` su un clippy fallito: clippy
esce 1, `tail` esce 0. Le catene sopra usano `&&` per questo: eseguile come sono scritte. Se
l'output è troppo lungo, tieni lo stato e taglia il testo a parte:

```bash
status=0
cargo clippy --all-targets --locked -- -D warnings > /tmp/clippy.txt 2>&1 || status=$?
tail -20 /tmp/clippy.txt
[ "$status" -eq 0 ]   # ← il verdetto, letto dal gate e non dal pager
```

`|| status=$?` e non `; status=$?`: con `set -e` attivo (il Passo 4 lo attiva) un comando
fallito fuori da una lista `||` chiude la shell prima che `tail` mostri il perché.

**Ogni PR esegue anche la suite di automazione**, comprese quelle solo di documentazione:

```bash
failed=0
for t in scripts/test/*.test.sh; do
  printf '== %s\n' "$t"
  bash "$t" || { echo "FAILED: $t"; failed=1; }
done
python3 .github/scripts/test_deepseek_review.py || failed=1
[ "$failed" -eq 0 ]  # ← lo stato di uscita del blocco È il verdetto; non togliere questa riga
```

Il glob è l'istruzione: non elencare i file a mano, la lista resterebbe indietro rispetto alla
cartella. L'ultima riga è portante: con `break` al posto del flag il ciclo uscirebbe 0 su un
test fallito.

Se modifichi una skill in `.claude/skills/`, rigenera gli adapter
(`bash scripts/sync-agent-skills.sh`) e committa le tre copie insieme.

La formattazione è il gate saltato più spesso e quello che arrossa più spesso la CI. Correggi con
`cargo fmt --all`, segui i messaggi di clippy, poi riesegui l'intero gate.

**Definition of Done (verificata prima della PR):**

- [ ] Formattazione pulita (`cargo fmt --check`)
- [ ] Lint pulito (`cargo clippy -D warnings`)
- [ ] Build backend (`cargo build --locked`) e frontend (`pnpm build`) riuscite
- [ ] Test verdi (`cargo test --locked`)
- [ ] Suite di automazione verde
- [ ] Nessuna stampa di debug nel codice di produzione
- [ ] Nessun segreto, token o chiave nel codice o nei test
- [ ] Chiave API solo nel backend, mai nel frontend né nei log
- [ ] Privacy di pubblicazione pulita: `check-publication-privacy.sh` sull'indice e su ogni
      commit del branch, `check-commit-privacy.sh` su autore e messaggi dei commit del branch

### Passo 7: commit, poi misura quello che il reviewer dovrà leggere

DeepSeek scrive il ragionamento nello stesso budget di output del verdetto: un diff troppo
grande può esaurirlo e non restituire niente. Il limite di `DEEPSEEK_MAX_DIFF_CHARS` è
**90,000** caratteri; oltre, il diff viene troncato e ogni file non letto diventa un finding che
blocca la PR finché qualcuno non lo legge e lo dichiara pubblicamente. Un umano può dare questo
riconoscimento; `/process-pr` può farlo solo con la sua sequenza documentata, datata e basata su
prove. Nessuno dei due è un esito da cercare apposta.

**Prima il commit, poi la misura.** `git diff "origin/$BASE_BRANCH"...HEAD` confronta commit:
su un albero non committato risponde `0` qualunque sia il lavoro in sospeso.

```bash
git add -A
git commit -m "<tipo-conventional-commit>: <descrizione concisa>

Implements #<numero-issue>

- <modifica principale>
- <modifica principale>"

# Quello che il reviewer vede davvero: il codice generato e i lockfile sono esclusi per
# nome, quindi escludili anche qui. `wc -m` e non `wc -c`: il limite è in caratteri.
# Due pathspec per lockfile: senza `:(glob)` un pathspec è ancorato alla radice, quindi
# `Cargo.lock` da solo non esclude `src-tauri/Cargo.lock`. Il reviewer confronta il
# basename a qualunque profondità (`is_generated_path` in .github/scripts/deepseek_review.py);
# scripts/test/diff-measure-parity.test.sh tiene allineate le due liste.
REVIEWABLE=$(git diff "origin/$BASE_BRANCH"...HEAD -- . \
  ':(exclude)gen/*' ':(exclude)*/gen/*' ':(exclude)*_generated.*' \
  ':(exclude)Cargo.lock' ':(exclude)*/Cargo.lock' \
  ':(exclude)go.sum' ':(exclude)*/go.sum' \
  ':(exclude)pnpm-lock.yaml' ':(exclude)*/pnpm-lock.yaml' | wc -m)
echo "reviewable diff: ${REVIEWABLE} characters (cap 90,000)"
```

**Misura contro la base su cui la parte verrà davvero aperta.** Una parte nata dalla parte
precedente e aperta su di essa misura solo sé stessa. Misurarla contro `develop` mentre la parte
precedente non è unita conta anche i commit di quella.

**Se devi cambiare la base di una PR, fallo prima del push.** Il cambio di base genera
`pull_request` con azione `edited`, che non è tra i tipi di attività predefiniti: da solo non
rilancia niente. Prima cambia la base, poi fai il push, così è il push a far partire la CI.

**Se la misura supera il limite, la stima era sbagliata e il lavoro va diviso prima di aprire
qualunque PR**, lungo un confine come quelli del Passo 5, in quante parti servono. Se le parti
non possono reggersi da sole, dillo e chiedi all'utente invece di aprire pezzi che non
compilano.

Annota nella PR la stima del Passo 5 accanto alla misura: due numeri rendono la stima
verificabile.

### Passo 8: push e PR

Prima del push i due controlli di privacy: il contenuto (l'indice e ogni commit che il branch
aggiunge, uno per uno) e i metadati dei commit (autore, committer e messaggio). Prima di creare la PR, il corpo. Un controllo fallito
blocca la pubblicazione anche con tutti i gate verdi: togli il dato prima di continuare, senza
ristamparlo in un altro messaggio. Il corpo di una PR descrive quello che hai appena fatto, e
l'hai fatto in una cartella con un nome: scrivi `<worktree>` o `<repo-root>`, mai il percorso.

```bash
# Dal worktree, con il commit del Passo 7 già fatto:
bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
git push -u origin HEAD

PR_BODY=$(mktemp)
trap 'rm -f "$PR_BODY"' EXIT
cat >"$PR_BODY" <<'EOF'
## Sintesi

Closes #<numero-issue>

### Modifiche
- <modifiche principali>

### Dimensione per la review
- Stima prima dell'implementazione: <stima e ragionamento>
- Diff misurato: <REVIEWABLE> caratteri

### Gate eseguiti
- [x] cargo fmt / clippy
- [x] build backend e frontend
- [x] test — tutti verdi
- [x] suite di automazione

### Verifica dei criteri di accettazione
<ogni criterio, spuntato oppure non spuntato con la spiegazione>

### Fuori scope (verificato)
<conferma che niente dell'elenco Fuori scope è stato toccato>
EOF

bash scripts/check-body-privacy.sh <"$PR_BODY"
gh pr create \
  --base "$BASE_BRANCH" \
  --title "<titolo-issue>" \
  --body-file "$PR_BODY"
```

**Per correggere titolo o corpo di una PR non usare `gh pr edit`.** Su `gh` 2.45.0 il suo
pre-fetch dei Projects classic fallisce prima di applicare qualunque flag. Usa
`bash scripts/gh-automation.sh pr-edit <pr> --title "<titolo>" --body-file <path>`, che scrive
via REST e rilegge i campi prima di dichiarare successo.

Spunta solo le caselle che hai eseguito davvero. Una casella vuota con una riga di motivazione è
utile; una spuntata e falsa avvelena ogni review successiva.

**Target della PR**: `develop` di default; un `--base` esplicito può puntare a un altro branch di
feature. Mai aprire o ridirigere una PR verso `main`, mai push diretto su `main`, mai merge verso
`main`. Il merge verso basi diverse da `main` è autorizzato, ma **non da questa skill**:
`/dev-issue` esce alla creazione della PR, prima che la CI abbia detto qualcosa.

### Passo 9: uscita

Riporta all'utente:

- nome del branch
- numero e URL della PR
- cartella del worktree
- che la review DeepSeek partirà da sola con `deepseek-pr-review.yml`

**NON** aspettare la CI, non controllare le review, non fare merge.

### Passo 10: rimuovere il worktree

```bash
# Torna al repo principale (ricavato, non scritto a mano)
MAIN_REPO=$(git rev-parse --path-format=absolute --git-common-dir)
MAIN_REPO=$(dirname "$MAIN_REPO")
cd "$MAIN_REPO"

git worktree remove "$WORKTREE_DIR"
git worktree prune
```

Conferma: "Worktree rimosso: `$WORKTREE_DIR`".

Se `git worktree remove` fallisce (modifiche non committate), avvisa l'utente e dai il comando
per la pulizia manuale. `node_modules` e `src-tauri/target` sono ignorati da git e non bloccano
la rimozione.

## Riferimenti

- Template delle issue: `.github/ISSUE_TEMPLATE/`
- Pipeline: `AGENTS.md`
- Gate (fonte di verità): `.github/workflows/ci.yml`
- Protezione dei branch: `.github/branch-protection.md`
- Helper: `scripts/gh-automation.sh`
