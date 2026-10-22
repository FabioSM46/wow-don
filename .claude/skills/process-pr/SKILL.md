---
name: process-pr
description: Manages PR feedback loops. Use to resolve CI failures and review feedback on open PRs.
argument-hint: <pr-number>
allowed-tools: Bash(gh *) Bash(git *) Bash(cargo *) Bash(rustup *) Bash(pnpm *) Bash(node *) Bash(python3 *) Bash(bash *) Bash(cd *) Bash(mkdir *) Bash(ls *) Bash(rm *) Bash(cp *) Bash(mv *) Bash(cat *) Bash(xargs *) Bash(sed *) Bash(awk *) Bash(tr *) Bash(head *) Bash(tail *) Bash(paste *) Bash(find *) Bash(sleep *) Bash(date *) Bash(dirname *) Bash(echo *) Bash(set *) Bash(source *) Bash(export *) Bash(jq *)
---

# process-pr — correzione di una PR

Trigger: `/process-pr <numero-pr>` oppure `/process-pr` (usa la PR del branch corrente).

## Scopo

Un ciclo di correzione per conflitti, CI rossa e feedback di review. Può avviarlo l'utente o un
orchestratore.

Il percorso passivo (`pr-labeler.yml`) copre il caso normale. Questa skill serve quando vuoi un
risultato adesso.

**Tutto il lavoro avviene in un `git worktree`**, mai nella cartella principale. Se esiste già un
worktree per il branch della PR (da un `/dev-issue` o `/process-pr` non ripulito), si riusa.

## Tempi reali — leggere prima di toccare il polling

Il job di review DeepSeek ha un budget di `DEEPSEEK_REQUEST_TIMEOUT_SECONDS` (2700) ×
(`DEEPSEEK_MAX_RETRIES` (1) + 1) = 90 minuti, sotto un `timeout-minutes: 100` con 10 minuti
riservati a setup e pubblicazione. I tempi reali dipendono dalla dimensione del diff; il limite
del diff è 90,000 caratteri, e una PR vicina al limite può usare buona parte del budget.
**Un'attesa più corta del budget non "controlla": scade e riporta uno stato vecchio.**
`DEEPSEEK_WAIT_SECONDS` qui sotto è 5700 (95 minuti), copre il budget e finisce prima del
limite del job; non abbassarlo senza cambiare prima il workflow.

Il budget di round è `MAX_ROUNDS: "1"` (in `.github/workflows/deepseek-pr-review.yml`). Una sola
review completa (Mode A) è automatica; le risposte nei thread (Mode B) non consumano budget e
continuano senza limite. Quindi l'esecuzione normale di questa skill è **un solo passaggio**.

I run `pull_request` usano il merge ref, quindi il workflow che gira è la copia del **branch
base**. Per diagnosticare il comportamento della review leggi la versione di
`deepseek-pr-review.yml` sulla base reale della PR.

## Workflow

### Passo 0: setup comune

```bash
set -euo pipefail
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
REPO_ROOT=$(git rev-parse --show-toplevel)
DEEPSEEK_WAIT_SECONDS=5700
POLL_INTERVAL=30
```

### Passo 1: numero della PR

Se l'utente l'ha dato, usalo. Altrimenti cerca la PR del branch corrente:

```bash
gh pr list --head "$(git branch --show-current)" --json number --jq '.[0].number'
```

Se non c'è nessuna PR, esci con un errore.

### Passo 2: creare o riusare il worktree e rilevare le aree

```bash
PR_META=$(gh pr view <pr-number> --json headRefName,baseRefName,state)
BRANCH=$(echo "$PR_META" | jq -er '.headRefName | select(length > 0)')
BASE_BRANCH=$(echo "$PR_META" | jq -er '.baseRefName | select(length > 0)')
[ "$(echo "$PR_META" | jq -r '.state')" = "OPEN" ] || exit 1
git fetch origin "$BRANCH" "$BASE_BRANCH"
REMOTE_HEAD=$(git rev-parse "origin/$BRANCH")

# Riusa un worktree già aperto per questo branch.
# substr() e non $2: i percorsi dei worktree possono contenere spazi.
WORKTREE_DIR=$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '
  /^worktree /{wt=substr($0,10)}
  /^branch /{if (substr($0,8)==b) {print wt; exit}}')

if [ -n "$WORKTREE_DIR" ]; then
  echo "Reusing existing worktree: $WORKTREE_DIR"
  cd "$WORKTREE_DIR"
  [ -z "$(git status --porcelain)" ] || {
    echo "Existing worktree is dirty; preserving it and stopping"
    exit 1
  }
  LOCAL_HEAD=$(git rev-parse HEAD)
  if [ "$LOCAL_HEAD" = "$REMOTE_HEAD" ]; then
    :
  elif git merge-base --is-ancestor "$LOCAL_HEAD" "$REMOTE_HEAD"; then
    git merge --ff-only "origin/$BRANCH"
  else
    echo "Existing worktree has local commits or diverged history; preserving it and stopping"
    exit 1
  fi
else
  WORKTREE_DIR="$(dirname "$REPO_ROOT")/wow-don-pr-<pr-number>"
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    [ "$(git rev-parse "refs/heads/$BRANCH")" = "$REMOTE_HEAD" ] || {
      echo "Local branch differs from origin; preserving it and stopping"
      exit 1
    }
    git worktree add "$WORKTREE_DIR" "$BRANCH"
  else
    git worktree add -b "$BRANCH" "$WORKTREE_DIR" "origin/$BRANCH"
  fi
  cd "$WORKTREE_DIR"
fi

pnpm install --frozen-lockfile
(cd src-tauri && cargo fetch --locked)

[ "$(git rev-parse --show-toplevel)" = "$WORKTREE_DIR" ] || { echo "NOT in worktree — abort"; exit 1; }
```

**Rileva ogni area toccata, non solo la prima.** La classificazione è la stessa di
`scripts/changed-areas.sh`, che è anche quella della CI:

```bash
AREAS=$(gh pr view <pr-number> --json files --jq '.files[].path' | bash scripts/changed-areas.sh)
echo "$AREAS"   # backend=true|false, frontend=true|false
```

**Ogni PR esegue la suite di automazione**, anche quelle solo di documentazione. I test
dell'automazione leggono file di tutto il repo, quindi nessun selettore di percorso li esenta.
Se una PR modifica una skill, rigenera anche gli adapter.

**Gate — rispecchiano `.github/workflows/ci.yml`. Eseguine uno per area rilevata:**

Esegui ogni catena così com'è e prendi il suo exit status come verdetto. Passarla a `head` o
`tail` restituisce il successo del pager invece di quello del gate; `/dev-issue` Passo 6 spiega
il perché.

| Area | Comando (da `$WORKTREE_DIR`) |
| ---- | ---------------------------- |
| `backend` | `cd src-tauri && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo build --locked && cargo test --locked` |
| `frontend` | `pnpm install --frozen-lockfile && pnpm build` |
| Ogni PR | tutta la suite `scripts/test/*.test.sh` (con il glob, mai riscrivendo la lista) più `python3 .github/scripts/test_deepseek_review.py`, con un flag di fallimento così che un test rosso sia lo stato di uscita del blocco, come in `/dev-issue` Passo 6 |

La formattazione è il gate saltato più spesso e quello che arrossa più spesso la CI.

#### Passo 2b: allinearsi alla base attuale

Fallo a ogni esecuzione. Se la punta attuale della base non è antenata della testa della PR,
fai merge della base nella testa senza rebase né force-push, risolvi i conflitti, esegui tutti i
gate delle aree coinvolte e fai push. Questo invalida anche una CI vecchia su una PR figlia
tecnicamente mergiabile dopo che la sua base di feature è avanzata.

```bash
BASE_HEAD=$(git rev-parse "origin/$BASE_BRANCH")
if ! git merge-base --is-ancestor "$BASE_HEAD" HEAD; then
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || exit 1
  git merge --no-ff --no-commit "origin/$BASE_BRANCH"
  # Risolvi i conflitti; se l'intento è ambiguo, interrompi e fermati senza pubblicare.
  # Esegui tutti i gate coinvolti, poi:
  git add -A
  git commit -m "fix: reconcile ${BASE_BRANCH} on PR #<pr-number>"
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi
```

Dopo questo push serve una CI nuova. I commit della base sono già stati revisionati nelle loro
PR, quindi non aspettare una review automatica che non può partire a budget esaurito.

### Passo 3: stato della PR

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

Valuta l'intera regola congelata di READY TO MERGE e stampa una riga `[FAIL]` per ogni
condizione non soddisfatta. Le condizioni sono **sette**, tutte necessarie:

1. Thread di review non risolti (e non obsoleti) = 0
2. Nessuna review che richiede modifiche
3. Nessun check di CI fallito
4. Nessun check di CI in corso
5. Nessun check obbligatorio *mancante*: il check stabile `ci-gate` deve aver girato con successo
6. `mergeable == MERGEABLE`
7. DeepSeek ha finito (verdetto pulito, round esauriti o esenzione `NO_DEEPSEEK_REVIEW`), senza
   finding non letti nel corpo di una review (si chiudono con la label `DEEPSEEK_REVIEW_READ`)

Le condizioni 5 e 6 esistono perché **una PR in conflitto non esegue nessun check**: senza niente
di rosso, una lettura ingenua la direbbe verde. Un conflitto è una correzione da fare, non
un'attesa: il Passo 2b fa merge della base attuale nella testa della PR così che la CI possa
ripartire.

L'helper fallisce in modo chiuso: un valore illeggibile conta come fallimento, mai come successo.

### Passo 4: gestire il round di review DeepSeek (nel worktree)

Con `MAX_ROUNDS: "1"` è un **solo passaggio**. Ripeti 4a–4e solo se `max_rounds` viene alzato
nel workflow o dopo un `pr-deepseek-force-review` esplicito. Limite di sicurezza: **5 iterazioni**.

#### 4a — Leggere lo stato di DeepSeek

```bash
ROUNDS=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
if ! echo "$ROUNDS" | jq -e '.' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds returned invalid JSON. Raw output: ${ROUNDS:0:200}"
  exit 1
fi
# L'helper riporta errori di configurazione/API come {"error":...} con uscita non zero,
# quindi un conteggio a zero significa sempre "nessuna review", mai "lettura fallita".
if echo "$ROUNDS" | jq -e '.error' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds failed: $(echo "$ROUNDS" | jq -r '.error')"
  exit 1
fi
REVIEW_COMPLETE=$(echo "$ROUNDS" | jq -r '.review_complete')
ROUNDS_EXHAUSTED=$(echo "$ROUNDS" | jq -r '.review_rounds_exhausted')
BOT_REVIEW_COUNT=$(echo "$ROUNDS" | jq -r '.bot_review_count')
PREV_REVIEW_ID=$(echo "$ROUNDS" | jq -r '.latest_review_id')
ROUND_CAP=$(echo "$ROUNDS" | jq -r '.max_rounds')
echo "DeepSeek state: rounds=$BOT_REVIEW_COUNT/$ROUND_CAP complete=$REVIEW_COMPLETE exhausted=$ROUNDS_EXHAUSTED"
```

**Guardia**: se `review_complete=true` o `review_rounds_exhausted=true` non c'è un round da
aspettare. Salta 4b e vai a 4c: un round già arrivato ha comunque thread da gestire.

#### 4b — Aspettare la review (solo se non ne è arrivata nessuna)

Solo quando `PREV_REVIEW_ID` è `0`.

```bash
START_TIME=$(date +%s)
while true; do
  sleep "$POLL_INTERVAL"

  # Il controllo della scadenza va PRIMA di ogni guardia che può fare `continue`:
  # un helper che fallisce sempre altrimenti girerebbe all'infinito.
  ELAPSED=$(($(date +%s) - START_TIME))
  if [ "$ELAPSED" -ge "$DEEPSEEK_WAIT_SECONDS" ]; then
    echo "Timeout (${DEEPSEEK_WAIT_SECONDS}s) waiting for DeepSeek — the job may be approaching its own 100-minute cap."
    echo "Check: gh run list --workflow=deepseek-pr-review.yml --limit 3"
    break
  fi

  CURRENT=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
  if ! echo "$CURRENT" | jq -e '.' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds returned invalid JSON, retrying... (${CURRENT:0:200})"
    continue
  fi
  if echo "$CURRENT" | jq -e '.error' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds failed ($(echo "$CURRENT" | jq -r '.error')), retrying..."
    continue
  fi
  NEW_ID=$(echo "$CURRENT" | jq -r '.latest_review_id')

  # Prima un nuovo ID di review: anche una review con review_complete=true porta feedback.
  if [ "$NEW_ID" != "0" ] && [ "$NEW_ID" != "$PREV_REVIEW_ID" ]; then
    echo "DeepSeek review arrived (id=$NEW_ID)"
    PREV_REVIEW_ID="$NEW_ID"
    BOT_REVIEW_COUNT=$(echo "$CURRENT" | jq -r '.bot_review_count')
    break
  fi

  if [ "$(echo "$CURRENT" | jq -r '.review_complete')" = "true" ]; then
    echo "DeepSeek approved with no issues found"
    break
  fi

  echo "Waiting for DeepSeek review... (elapsed: ${ELAPSED}s / ${DEEPSEEK_WAIT_SECONDS}s)"
done
```

Un timeout qui è un segnale vero. Un job cancellato dal limite di 100 minuti non produce **nessun
output**, e `pr-status-json` conta `CANCELLED` come fallito: la PR resta su `needs-work` senza
niente nel log che spieghi perché. Se è successo, dillo esplicitamente.

#### 4c — Gestire il feedback del round

1. **Leggi i thread non risolti e non obsoleti**:

   ```bash
   bash scripts/gh-automation.sh pr-comments <pr-number>

   gh api graphql --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviewThreads(first:50){nodes{id isResolved isOutdated comments(first:2){nodes{databaseId body}}}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number> \
     --jq '.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false and .isOutdated == false) | {id, comments: [.comments.nodes[] | {databaseId, body}]}'
   ```

2. **Correggi ogni thread** (file sotto `$WORKTREE_DIR/`):
   - bug o suggerimento valido → implementalo
   - domanda → rispondi
   - dettaglio da meno di 5 minuti → correggilo e basta
   - fuori scope → rispondi spiegando perché; non ignorarlo in silenzio

   **Ordine di pubblicazione:** prepara le risposte nei punti 3–5, ma non inviare risposte, non
   risolvere thread, non pubblicare la verifica del corpo e non scrivere `DEEPSEEK_REVIEW_READ`
   finché il Passo 4e non ha fatto push di ogni correzione e verificato la testa remota. Una
   correzione che esiste solo nel worktree non è arrivata.

3. **Prepara una risposta per ogni thread**, anche quelli su cui non hai agito. Annota ID del
   commento, ID del thread e testo per il Passo 4f; nessuna scrittura su GitHub per ora.

4. **Prepara la risoluzione** solo dei thread la cui correzione sarà arrivata o il cui punto è
   davvero gestito. Annota la decisione per il Passo 4f.

   Un thread il cui suggerimento hai **respinto** si può risolvere, ma solo dopo una risposta
   che riporta le prove: quella risposta è l'intera traccia di audit. Mai risolvere un thread a
   cui non hai risposto.

5. **I finding nel corpo della review** non creano thread e non si possono risolvere. Leggi i
   corpi completi delle review DeepSeek recenti, non solo i thread, e leggi ogni finding prima di
   decidere se riconoscerli:

   ```bash
   ACK_STATE=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>)
   ACK_REVIEW_ID=$(echo "$ACK_STATE" | jq -er '.latest_review_id | select(. != 0)')
   if ! BODY_REVIEWS=$(gh api graphql \
     --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviews(last:100,states:[APPROVED,CHANGES_REQUESTED,COMMENTED]){nodes{databaseId submittedAt state author{login} body}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number>); then
     echo "Could not fetch DeepSeek review bodies; acknowledgement is blocked"
     exit 1
   fi
   echo "$BODY_REVIEWS" | jq --arg bot "${DEEPSEEK_BOT_USER:-github-actions[bot]}" '
     def canon: sub("\\[bot\\]$"; "");
     .data.repository.pullRequest.reviews.nodes[]
     | select((.author.login // "" | canon) == ($bot | canon))
     | select((.body // "") != "")
     | {databaseId, submittedAt, state, body}'
   ```

   Correggi nel codice ogni punto valido. Un punto respinto richiede prove concrete dal diff, da
   un test o da un comportamento misurato: un disaccordo senza prove non è una risposta. Se un
   finding non è chiaro o non ha prove né in un senso né nell'altro, lascialo non riconosciuto e
   segnala il blocco.

   Annota una risposta per ogni finding del corpo per il Passo 4f. Mai usare
   `NO_DEEPSEEK_REVIEW`: quell'esenzione resta solo umana.

#### 4d — Correggere la CI

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

Se la CI fallisce, leggi i log (`gh run view <run-id> --log-failed`), correggi, poi esegui il gate
del Passo 2 per **ogni** area coinvolta prima del push.

Se il problema è di infrastruttura (runner, mirror di pacchetti, token scaduto), segnalalo e
fermati. Non nasconderlo con modifiche al codice.

#### 4e — Commit e push

Se il feedback richiede di correggere titolo o corpo della PR, **non usare `gh pr edit`**: su
`gh` 2.45.0 fallisce prima di applicare i flag. Usa
`bash scripts/gh-automation.sh pr-edit <pr> --title "<titolo>" --body-file <path>`.

```bash
cd "$WORKTREE_DIR"
git add -A
if git diff --cached --quiet; then
  echo "No source changes to commit"
else
  git commit -m "fix: address feedback on PR #<pr-number>"
  git fetch origin "$BRANCH"
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || {
    echo "PR head moved; preserve local commits and reconstruct state"
    exit 1
  }
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi

# Confirm the PR is still open at REMOTE_HEAD. Only then execute the prepared
# replies/resolutions and body acknowledgement sequence from steps 3–5.
CURRENT_PR=$(gh pr view <pr-number> --json state,headRefOid)
[ "$(echo "$CURRENT_PR" | jq -r '.state')" = "OPEN" ] || exit 1
[ "$(echo "$CURRENT_PR" | jq -r '.headRefOid')" = "$REMOTE_HEAD" ] || exit 1
```

**MAI eseguire `git add` / `git commit` / `git push` dalla cartella principale.** Sempre da
`$WORKTREE_DIR`.

A budget di round esaurito, questo push **non** fa partire un'altra review. Se serve davvero un
altro passaggio:

```bash
bash scripts/gh-automation.sh pr-deepseek-force-review <pr-number> [ref]
```

`ref` è `develop` di default e deve essere un branch che contiene il bypass, perché il run
lanciato esegue la definizione del workflow di quel ref.

#### 4f — Pubblicare le risposte dopo il push

Solo dopo che il Passo 4e ha verificato la testa remota, pubblica ogni risposta preparata e poi
risolvi il suo thread:

```bash
gh api "repos/$REPO/pulls/<pr-number>/comments/<comment-databaseId>/replies" \
  --field body="<risposta che nomina la correzione pubblicata o le prove del rifiuto>" || exit 1
gh api graphql \
  -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id isResolved}}}' \
  -f id="<THREAD_ID>" || exit 1
```

Per i finding del corpo pubblica una risposta per finding (corretto, con file/test, oppure
respinto, con le prove). È la traccia di audit pubblica. Poi rileggi `latest_review_id`: deve
essere ancora uguale a `ACK_REVIEW_ID`, altrimenti ripeti questo passo dalla lettura dei corpi.
Rinnova `DEEPSEEK_REVIEW_READ` solo dopo che la traccia esiste, trattando la lettura della label
a tre stati (presente/assente/illeggibile), e verifica sia la label nuova sia le postcondizioni:

```bash
ACK_COMMENT='<un punto per finding del corpo DeepSeek, con ID della review e prove>'
gh pr comment <pr-number> --body "$ACK_COMMENT" || exit 1
LATEST_BEFORE_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
[ "$LATEST_BEFORE_ACK" = "$ACK_REVIEW_ID" ] || exit 1
if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ || exit $?
  if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
    echo "Stale acknowledgement label is still present"
    exit 1
  else
    LABEL_RC=$?
    [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
  fi
else
  LABEL_RC=$?
  [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
fi
bash scripts/gh-automation.sh pr-label <pr-number> add DEEPSEEK_REVIEW_READ || exit $?
if ! bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  echo "Could not verify the fresh acknowledgement label"
  exit 1
fi
LATEST_AFTER_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
UNREAD_AFTER_ACK=$(bash scripts/gh-automation.sh pr-status-json <pr-number> \
  | jq -er '.deepseek_unread_findings')
if [ "$LATEST_AFTER_ACK" != "$ACK_REVIEW_ID" ] || [ "$UNREAD_AFTER_ACK" != "0" ]; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ
  exit 1
fi
```

Una review più recente resta non letta perché il suo timestamp segue la label. Se una risposta
non è capita o non ha prove, lascia la label assente. Mai applicare `NO_DEEPSEEK_REVIEW`.

### Passo 5: verifica finale

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
bash scripts/gh-automation.sh pr-check-label <pr-number> "READY TO MERGE"
```

`pr-labeler.yml` applica la label quando la CI finisce, nel giro ogni sei ore e su dispatch
manuale. Se `pr-status` dice che la regola è soddisfatta ma la label manca, il labeler non è
ancora passato.

### Passo 6: rimuovere il worktree

```bash
MAIN_REPO=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
cd "$MAIN_REPO"

git worktree remove "$WORKTREE_DIR"
git worktree prune
```

Se `git worktree remove` fallisce, avvisa l'utente e dai il comando per la pulizia manuale.

### Passo 7: report

```
PR #<numero>: <titolo>
├── Mergeable:   ✅ nessun conflitto
├── CI:          ✅ ci-gate eseguito e verde
├── DeepSeek:    ✅ verdetto pulito / round esauriti
├── Thread:      ✅ 0 non risolti
├── Corpo:       ✅ nessun finding non letto (oppure: N non riconosciuti, con il motivo)
├── Label:       READY TO MERGE
└── Stato:       Pronta per un merge autonomo verso una base diversa da main
```

Riporta ogni riga da quello che `pr-status` ha stampato davvero.

**Non fare merge da questa skill.** È una separazione di responsabilità: la decisione di merge
verso basi diverse da `main` spetta a chi legge il risultato finito e i vincoli d'ordine della PR.

## Guardrail

- **Solo worktree**: modifiche, git e gate dentro il worktree, mai nella cartella principale.
- **Verifica il worktree**: prima di ogni modifica, `git rev-parse --show-toplevel` deve essere
  `$WORKTREE_DIR`.
- Un passaggio di default (`MAX_ROUNDS: "1"`); limite di 5 iterazioni se il budget viene alzato.
- Esegui i gate di **ogni** area toccata.
- Se la CI fallisce per infrastruttura, segnalalo e fermati.
- **Non fare polling su `mergeable` e non bloccarti su di esso.** Una PR **già unita** lo riporta
  `null` per sempre, quindi aspettare che cambi non è lento, è impossibile. `pr-status` lo
  riprova internamente e il labeler lo rilegge a ogni passaggio. Riporta quello che hai misurato
  ed esci.
- **Prima di aspettare qualcosa su una PR, controlla che sia ancora aperta.**
- Se DeepSeek non arriva entro `DEEPSEEK_WAIT_SECONDS` (5700), segnalalo come probabile timeout
  del job ed esci dal ciclo: non trattarlo come "nessun finding".
- **Limiti GraphQL**: un'attesa è al massimo 190 richieste (5700s ÷ 30s) da circa 3–5 punti, sotto
  i 950 punti. GitHub ne concede 5000 all'ora. Evita cicli forzati in parallelo su più PR.
- MAI push diretto su `main` e mai merge di una PR verso `main`: sono solo umani. Il merge verso
  basi diverse da `main` è autorizzato tramite
  **`bash scripts/gh-automation.sh pr-merge <pr> --head <sha-osservato> --base-head
  <sha-base-osservato>`**: rifiuta una base `main`, fallisce chiuso su una base illeggibile e
  rifiuta una testa o una base che si sono mosse dopo la lettura. Leggi il corpo della PR prima
  del merge: un ordine dichiarato rispetto a un'altra PR vincola chi fa il merge, e la regola
  congelata non lo vede.
- Mai force-push o rebase senza istruzione esplicita dell'utente. I conflitti si risolvono con un
  merge della base nella testa della PR, preservando la storia.
- Esegui sempre i gate in locale prima di pubblicare correzioni.
- Applica `DEEPSEEK_REVIEW_READ` solo con la sequenza lettura/risposta/audit pubblico/scrittura
  fresca dei passi 4c e 4f. Mai `NO_DEEPSEEK_REVIEW`.

## Riferimenti

- Helper: `scripts/gh-automation.sh` (`pr-status`, `pr-deepseek-rounds`, `pr-deepseek-force-review`)
- Reviewer DeepSeek: `.github/workflows/deepseek-pr-review.yml` / `.github/scripts/deepseek_review.py`
- Monitor passivo: `.github/workflows/pr-labeler.yml`
- Gate (fonte di verità): `.github/workflows/ci.yml`
- Protezione dei branch: `.github/branch-protection.md`
