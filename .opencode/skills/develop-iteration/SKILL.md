---
name: develop-iteration
description: Use ONLY when the user explicitly requests /develop-iteration. Develops every committed issue in an iteration, parallelizes independent work, processes PR feedback, and autonomously merges ready PRs into non-main bases.
compatibility: opencode
metadata:
  opencode/autoinvoke: "false"
---


# develop-iteration — consegna di un'iterazione

Trigger: `/develop-iteration <numero-iterazione-o-milestone>` oppure `/develop-iteration` (usa
l'iterazione attiva).

## Scopo

Consegna le issue già assegnate a un'iterazione. Questa skill **non** sceglie lo scope
dell'iterazione: quella decisione è di `/scrum-master iteration-plan` e dell'utente. Coordina un
agente per issue, usa `/dev-issue` per l'implementazione e `/process-pr` per le correzioni, e fa
il merge di ogni PR che soddisfa la regola congelata.

L'esecuzione si può riprendere. Milestone, issue, branch, PR, check, review e commenti di GitHub
sono lo stato: non affidarti mai a una coda in memoria come unica traccia dell'avanzamento. A una
ripartenza, ricostruisci il piano da GitHub e continua senza duplicare branch o PR.

## Confini non negoziabili

- Mai creare, ridirigere o unire una PR con base `main`.
- Mai push diretto su `main` o su `develop`.
- Il merge può andare verso `develop` o verso qualunque altro branch che non sia `main`. I branch
  di feature e i loro sotto-branch sono topologia normale, non una pulizia eccezionale.
- Ogni merge passa da `bash scripts/gh-automation.sh pr-merge <pr> --head <sha-osservato>
  --base-head <sha-base-osservato>`. L'helper rifiuta `main` e una testa o una base cambiate, ma
  non decide se la PR è pronta: il controllo fresco del Passo 5 è obbligatorio.
- `NO_DEEPSEEK_REVIEW` resta solo umana. I finding nel corpo di una review DeepSeek si
  riconoscono solo con la sequenza di `/process-pr` (lettura, decisione, risposta pubblica,
  label fresca `DEEPSEEK_REVIEW_READ`).
- Le cerimonie delle iterazioni restano con l'umano nel ciclo. Fermati dopo la consegna: non
  eseguire in automatico né backlog refinement né iteration planning.
- Le regole di `AGENTS.md` valgono per ogni agente: worktree isolati, nessuna lettura di `.env`,
  nessuna dipendenza nuova senza richiesta esplicita, controlli di privacy prima di ogni push e di
  ogni corpo pubblicato.

## 1. Risolvere l'iterazione e ricostruire lo stato

Con un argomento, risolvilo in **esattamente una** milestone aperta. Accetta il numero della
milestone, `Iteration N` o `Sprint N`. Senza argomento usa la stessa regola di
`iteration-advance`: la milestone aperta con la sequenza **più bassa**, letta dal titolo e in
mancanza dal numero della milestone. Se due milestone aperte hanno la stessa sequenza più bassa,
fermati e nominale entrambe: scegliere è una decisione di prodotto.

Leggi ogni issue non di cerimonia (senza label `ceremony`) assegnata alla milestone, **comprese
quelle chiuse**, e trova ogni PR che la cita. Per ogni issue registra:

- stato e campi obbligatori di `/dev-issue` (letti dal **corpo**, come li legge `/dev-issue`);
- dipendenze dichiarate (`### Dipendenze`) e vincoli d'ordine scritti nel corpo;
- branch e PR esistenti, con la **base reale** della PR;
- stato di CI, DeepSeek, thread di review e mergeabilità;
- l'area (`backend`, `frontend`, `shared`) e i file probabili da `### Riferimenti nel codice` e
  `### Contesto tecnico`.

Il lavoro già unito è completo. Riusa le PR aperte e i worktree esistenti invece di aprirne di
duplicati. Una PR chiusa senza merge, o un branch il cui legame con la issue è ambiguo, è una
condizione di stop per quella issue, non un permesso di indovinare.

Una dipendenza **fuori** dall'iterazione che non è ancora su `develop` blocca la issue che ne
dipende: segnalala, non anticiparla.

## 2. Costruire il grafo delle dipendenze e dei branch prima di assegnare il lavoro

Costruisci un grafo diretto aciclico da: dipendenze esplicite delle issue, l'ordine tra aree di
`AGENTS.md` (**prima il backend** con i tipi `serde` e il comando Tauri, **poi il frontend** che
lo usa), corpi delle PR e relazioni di base tra PR già aperte. Una PR verso un altro branch di
feature dipende da quel branch e deve essere unita prima che la PR finale del branch padre possa
salire.

Scegli la base di ogni issue nuova:

- `develop` per una issue indipendente (il default);
- il branch di feature pertinente quando la issue è una figlia di un lavoro assemblato lì
  apposta;
- la base reale di ogni PR già esistente, senza cambiarla;
- rifiuta `main` in fase di piano e ricontrollalo subito prima di creare la PR e prima del merge.

Passa una base diversa dal default in modo esplicito all'agente della issue:
`/dev-issue <issue> --base <branch>`. La base fa parte del piano: non ridirigere in silenzio una
PR esistente per semplificare il grafo.

Quando più PR figlie hanno bisogno di una nuova base aggregata, creala senza aprire la PR padre:

1. Scegli un nome `feature/<slug-descrittivo>` (mai `main`) e verifica che non esista già.
2. In un worktree temporaneo `<cartella-padre-del-repo>/wow-don-issue-iteration-<N>-aggregate`,
   crea il branch da `origin/develop` aggiornato, verifica la radice del worktree, fai push del
   branch, poi rimuovi il worktree.
3. Passa quella base agli agenti delle issue figlie.
4. Quando ogni PR figlia prevista è unita, crea la PR aggregata dal branch di feature verso
   `develop`. Controlla il corpo completo con `scripts/check-body-privacy.sh` prima di
   pubblicarlo, metti `Closes #N` per ogni issue contenuta, poi verifica base e corpo su GitHub.

Usa una base aggregata solo quando la PR finale combinata resta plausibilmente sotto il limite di
90,000 caratteri della review e la funzionalità si legge bene come un tutto. Altrimenti usa PR
in ordine di dipendenza direttamente verso `develop`.

Aprire la PR padre solo dopo l'assemblaggio dà alla CI e al round automatico di DeepSeek il diff
finale combinato. Se una PR padre era già aperta quando il merge di una figlia ne ha cambiato la
testa, aspetta una CI nuova e chiedi esplicitamente un'ultima review DeepSeek della testa
assemblata (`bash scripts/gh-automation.sh pr-deepseek-force-review <pr>`) prima di
considerarla pronta.

## 3. Decidere l'ondata parallela

Il parallelismo è il default, ma solo per lavoro davvero indipendente. Due issue possono stare
nella stessa ondata quando valgono **tutte** queste condizioni:

- nessuna delle due raggiunge l'altra nel grafo delle dipendenze;
- nessuna produce un contratto pubblico (tipi serializzati, comando Tauri, formato di un file di
  stato) usato dall'altra;
- i loro riferimenti nel codice e le superfici che modificheranno non si sovrappongono in modo
  sostanziale (in questo repository `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`,
  `src-tauri/src/store.rs` e `src/api.ts` sono punti di contatto frequenti: due issue che li
  modificano entrambe sono di norma da serializzare);
- unire una delle due per prima non può invalidare i criteri di accettazione dell'altra;
- le basi previste esistono già e non aspettano un'altra ondata.

Quando le prove non bastano, serializza e di' perché. Non trasformare l'incertezza in lavoro
parallelo. Ricalcola il grafo dopo ogni merge: un confine scoperto durante l'implementazione o un
corpo di PR possono aggiungere un vincolo d'ordine.

Usa lo **strumento Task** per avviare esattamente un agente proprietario per ogni issue dell'ondata
pronta, fino agli slot di concorrenza disponibili. Avvia insieme l'intero insieme massimo così
che giri in parallelo; le altre issue sicure aspettano l'ondata successiva. Dai a ogni agente il
numero della issue, la base prevista, le dipendenze rilevanti e questa istruzione:

> Esegui `/dev-issue <issue> --base <base-prevista>` e seguila fino in fondo. Riporta ogni PR
> creata (comprese quelle divise o impilate), con base, branch e URL di ognuna, e ogni nuova
> dipendenza scoperta. Non fare merge. Resti l'agente proprietario per il lavoro successivo di
> `/process-pr` su quelle PR.

Mai due agenti sulla stessa issue. Se una issue va divisa in più PR, il suo agente ne resta
proprietario mentre l'orchestratore aggiunge quelle PR e i loro archi di base al grafo. Quando
l'agente proprietario ha già restituito il suo report, il lavoro successivo gli arriva con
`SendMessage` (stesso agente, contesto intatto), non con un agente nuovo.

## 4. Sorvegliare e correggere l'insieme delle PR aperte

Quando un'ondata di implementazione torna, sorveglia tutte le sue PR aperte come un insieme.
Prima di aspettare una PR, verifica che sia ancora aperta. Una CI in corso o una prima review
DeepSeek ancora in corso sono un'attesa, non un motivo per chiamare `/process-pr`. La review
DeepSeek può durare decine di minuti (vedi "Tempi reali" in `/process-pr`).

**Sorveglia l'insieme con una sola sorveglianza persistente costruita su `pr-status-json`.**
Avviala appena esiste la prima PR dell'esecuzione: in Claude Code con lo strumento `Monitor` e
`persistent: true`, in un altro runtime con il ciclo in background equivalente. Non aspettare
con `sleep` in primo piano: tra un evento e l'altro il controllo torna all'utente, e ogni riga
emessa ti risveglia. Agisci su ogni riga: `ready:true` va al Passo 5; thread, finding non letti,
CI rossa o `CONFLICTING` vanno al `/process-pr` dell'agente proprietario; solo uno stato in
corso aspetta.

Imposta `ISSUES` con i numeri delle issue dell'iterazione, che ogni branch di `/dev-issue` porta
nel nome, e `EXTRA_PRS` con i numeri delle PR il cui branch non porta il numero di una issue (la
PR aggregata `feature/<slug>`, vuoto se non ce ne sono):

```bash
cd <repo-root>
ISSUES='<issue>|<issue>|<issue>'
EXTRA_PRS='<pr>|<pr>'
declare -A last
while true; do
  if ! all=$(gh pr list --state open --limit 100 --json number,headRefName,headRefOid \
      --jq '.[] | "\(.number) \(.headRefOid[:7]) \(.headRefName)"'); then
    echo "watch: gh pr list failed"; sleep 60; continue
  fi
  prs=$(printf '%s\n' "$all" | grep -E "/(${ISSUES})-|^(${EXTRA_PRS:-none}) " || true)
  while read -r n sha _; do
    [ -n "$n" ] || continue
    s=$(bash scripts/gh-automation.sh pr-status-json "$n" 2>/dev/null | jq -c '{ready:.ready_to_merge,
      pending:.ci_pending, failing:.ci_failing, gate:.required_check_state,
      threads:.unresolved_threads, unread:.deepseek_unread_findings,
      ds_done:(.deepseek_review_complete or .deepseek_rounds_exhausted), mergeable:.mergeable}' 2>/dev/null) || s=""
    [ -n "$s" ] || s="status-unreadable"
    if [ "${last[$n]:-}" != "$sha $s" ]; then last[$n]="$sha $s"; echo "PR #$n @$sha $s"; fi
  done <<<"$prs"
  sleep 60
done
```

Emette ogni PR corrispondente una volta al primo passaggio e di nuovo quando cambiano la testa o
lo stato; uno stato che non si legge esce come `status-unreadable`, mai come silenzio. Ogni
lettura che può fallire è protetta (`|| continue`, `|| true`, `|| s=""`, `${last[$n]:-}`), così
il ciclo mantiene questa promessa anche sotto `set -euo pipefail`: un'assegnazione fallita non
protetta chiuderebbe il ciclo senza una riga, che è proprio il silenzio che questa sorveglianza
deve evitare. Il filtro sui branch si àncora a `/<issue>-` e non dice niente sul prefisso
(`feature/`, `fix/`, `refactor/`). **Guarda il primo passaggio prima di fidarti:** una
sorveglianza che non ha stampato niente per una PR che esiste è rotta, non tranquilla; finché non
ha stampato, leggi `pr-status-json` a mano dopo ogni ondata.

Quando una PR aggregata nasce dopo l'avvio, ferma la sorveglianza (`TaskStop`) e riavviala con
il suo numero in `EXTRA_PRS`. A consegna finita fermala: un ciclo persistente non si chiude da
solo.

**Mai costruire la sorveglianza su `gh pr checks --json`.** Il `gh` minimo del repository
(2.18.0, `GH_MIN_VERSION` in `scripts/gh-automation.sh`) non ha `--json` su `pr checks`: la
chiamata esce con `unknown flag: --json`, e un ciclo che scarta stderr legge l'output vuoto come
"ancora nessun check" e salta la PR a ogni passaggio. In voxelheim-v2 (Iteration 60) proprio un
ciclo così è rimasto muto per ore mentre due PR erano `ready_to_merge`. `pr-status-json` legge lo
stesso rollup della regola congelata, quindi la sorveglianza e la decisione di merge non possono
divergere.

Quando una PR ha CI rossa da correggere, thread DeepSeek non risolti, finding non letti nel corpo
o `mergeable == CONFLICTING`, manda al suo agente proprietario un compito: eseguire
`/process-pr <pr>`. `ci-gate` assente insieme a `CONFLICTING` è una correzione di conflitti, non
un'attesa della CI: nessun workflow `pull_request` può partire finché non esiste il merge ref.
Tieni un agente per issue anche quando la issue ha più PR. Non eseguire cicli forzati in
parallelo su più PR della stessa issue, ed evita il polling DeepSeek concorrente non necessario,
per i limiti di frequenza di GraphQL.

Salva il conteggio delle correzioni in commenti pubblici sulla PR, così una ripartenza non
azzera il limite di sicurezza. Prima di ogni ciclo forzato conta i commenti con il marcatore
`<!-- develop-iteration:remediation:pr-N:attempt-K -->`, rifiuta il tentativo quattro, controlla
il nuovo commento con `scripts/check-body-privacy.sh` e pubblicalo prima di mandare l'agente.
Rileggi lo stato dopo ogni push. Ferma prima quella PR quando `/process-pr` riporta un guasto
dell'infrastruttura, un finding poco chiaro senza una decisione basata su prove, un rebase non
sicuro o un'altra condizione che richiede il giudizio dell'utente. Mai forzare un round DeepSeek
in più solo per far muovere il ciclo: il limite è **un** round automatico per PR (`MAX_ROUNDS`).

## 5. Unire le foglie pronte in ordine di dipendenza

Sono candidate solo le PR foglia le cui dipendenze sono già unite. Subito prima di ogni merge:

1. Rileggi stato della PR, `headRefOid`, branch base e **corpo completo**. Leggi lo SHA della
   base dalla forma REST della PR (`gh api "repos/$REPO/pulls/<pr>" --jq '.base.sha'`), perché
   il `gh` minimo non espone `baseRefOid` in `gh pr view --json`. Tieni i due SHA come
   `OBSERVED_HEAD` e `OBSERVED_BASE_HEAD`.
2. Verifica che sia ancora aperta e che la base non sia vuota e non sia `main`.
3. Verifica che la testa contenga la punta attuale della base
   (`git merge-base --is-ancestor <punta-base> <testa>` dopo un `git fetch`). Se non la contiene,
   la PR non è candidata: vedi "Dopo un merge" qui sotto.
4. Rispetta ogni vincolo d'ordine scritto nella sua issue e nel corpo della PR.
5. Esegui `bash scripts/gh-automation.sh is-ready-to-merge <pr>` sulla testa attuale.
6. Se riesce, esegui
   `bash scripts/gh-automation.sh pr-merge <pr> --head "$OBSERVED_HEAD" --base-head "$OBSERVED_BASE_HEAD"`.

`READY TO MERGE` è una prova utile ma può essere in ritardo rispetto allo stato attuale: non
sostituire mai il passo 5 con una label vecchia. Al contrario, una valutazione fresca della
regola congelata riuscita basta anche se il labeler a eventi non ha ancora messo la label.

I branch di feature non sono coperti dai ruleset di `develop` e `main`. Per loro il controllo
fresco della regola congelata è l'unica protezione sui check che l'orchestratore controlla: non
saltarlo mai.

### Dopo un merge

**Il branch che si è mosso invalida le prove delle PR che ci poggiano sopra**, anche se la loro
testa non è cambiata. Dopo un merge rileggi ogni PR aperta la cui **testa o base** è il branch
appena mosso, `develop` compreso. Ogni PR con base sul branch mosso passa dal suo agente
proprietario con `/process-pr`: il Passo 2b fa merge della nuova base nella testa senza rebase né
force-push, esegue i gate delle aree coinvolte e fa push; poi serve una CI nuova prima che la PR
torni candidata. Rileggi lo stato DeepSeek: se il round è già speso, non aspettare una review
automatica che non partirà mai, perché i commit della base sono stati revisionati nelle loro PR.
Una testa verde prima non dice niente su una base appena cambiata. Per una PR padre la cui testa
è il branch di feature vale la regola della testa assemblata del Passo 2, compresa la review
forzata esplicita.

**Su `develop` non c'è una verifica dopo il merge.** In questo repository la CI gira solo su
`pull_request` e non esiste un workflow di integrazione su `develop`: l'unica prova che due PR
stanno bene insieme è la CI della seconda, rifatta dopo aver incorporato la prima. Per questo il
riallineamento qui sopra vale anche per `develop` ed è obbligatorio: mai unire in `develop` una PR
la cui testa non contiene la punta attuale di `develop`, e unisci in `develop` **una PR alla
volta**, aspettando la CI fresca della successiva.

Una PR figlia unita solo in un branch di feature non chiude in modo affidabile la sua issue, e il
suo `Closes #N` non passa alla PR padre. Tieni quelle issue in sospeso finché il branch di
feature non arriva su `develop`, e assicurati che il corpo della PR padre porti `Closes #N` per
ogni issue di cui contiene i commit. Per una PR padre già aperta, scrivi il corpo completo
corretto in un file temporaneo, controllalo con `scripts/check-body-privacy.sh`, usa
`bash scripts/gh-automation.sh pr-edit <pr> --body-file <path>` (mai `gh pr edit`, che fallisce
con il `gh` 2.45), poi rileggi il corpo completo e tutti i vincoli d'ordine. Chiudi una issue a
mano solo dopo aver verificato che `develop` contiene i suoi commit.

## 6. Continuare finché l'iterazione non è consegnata

Dopo ogni merge ricostruisci il grafo issue/PR e avvia la successiva ondata sicura massima.
Continua finché ogni issue non di cerimonia della milestone è chiusa da una PR unita e non resta
aperta nessuna PR figlia o padre prevista.

Non chiudere la milestone e non eseguire le cerimonie create da `iteration-lifecycle.yml`: sono
decisioni di prodotto fuori dalla consegna. La chiusura dell'ultima issue fa partire da sola la
cerimonia di backlog refinement.

## Condizioni di stop

Quando puoi, ferma solo il ramo di lavoro colpito; ferma tutta l'esecuzione quando continuare
potrebbe rompere l'ordine o la base comune. Riporta lo stato esatto e il comando per riprendere
quando succede una di queste cose:

- campi obbligatori mancanti nel corpo di una issue, o dati ambigui su iterazione o dipendenze;
- una base `main` richiesta o già presente su una PR;
- una dipendenza fuori dall'iterazione non ancora su `develop`;
- un guasto dell'infrastruttura o uno stato di GitHub non leggibile;
- una modifica che supera il limite della review e non si può dividere;
- tre cicli di correzione falliti sulla stessa PR;
- una PR che `/process-pr` non riesce a riallineare alla base senza un giudizio sull'intento;
- una decisione che cambia lo scope dell'iterazione o richiede una nuova dipendenza.

## Report finale

Riporta la milestone, quale agente possiede quale issue, la topologia di branch e PR, i merge in
ordine, gli esiti di CI e DeepSeek, lo stato di `develop` dopo l'ultimo merge (l'ultima CI verde
della PR unita per ultima) e ogni lavoro fermato. Se tutte le issue sono arrivate, di' che la
consegna è completa e che la prossima transizione spetta al ciclo di vita delle iterazioni
guidato dal completamento.

## Riferimenti

- Implementazione di una issue: `/dev-issue`
- Correzione di una PR: `/process-pr`
- Stato congelato e merge protetto: `scripts/gh-automation.sh`
- Scelta dell'iterazione e ciclo di vita: `AGENTS.md`
- CI: `.github/workflows/ci.yml`
