# CLAUDE.md

Leggi e segui le istruzioni del file @AGENTS.md alla radice del progetto.

Il repository è privato ma **va trattato come pubblico**: potrebbe diventarlo, e tutto quello già
pubblicato resterebbe leggibile per sempre. Mai un indirizzo email reale, dati personali, nome
utente o percorso della macchina, hostname privato, segreto o credenziale in un file tracciato,
commit, branch, PR, review, log della CI o artifact. Usa l'indirizzo `noreply` di GitHub, valori
di esempio riservati e segnaposto come `<repo-root>`. Prima di ogni push esegui
`bash scripts/check-publication-privacy.sh <base> <head>` sul contenuto (indice e ogni commit
del branch) e `bash scripts/check-commit-privacy.sh <base> <head>` su autore e messaggi; prima di
pubblicare un corpo di issue o PR, `bash scripts/check-body-privacy.sh`.

La chiave CurseForge sta solo in `.env` (ignorato da git) e nel backend Rust. Gli agenti non
leggono mai `.env`; per sapere quali variabili servono usano `.env.example`. Niente firme degli
strumenti in commit e PR: nessun trailer `Co-Authored-By` di agenti, nessun "Generated with",
nessun link `claude.ai/code/session_…` né trailer `Claude-Session:`.

Riferimento rapido — le skill della pipeline:

- `/dev-issue <numero>` — implementa una issue GitHub dall'inizio alla fine (worktree → gate → PR)
- `/process-pr [numero]` — ciclo forzato su una PR aperta (feedback DeepSeek + correzioni CI)
- `/develop-iteration [iterazione]` — consegna le issue di una milestone (agenti paralleli → PR → merge)
- `/scrum-master feature-spec | backlog-refine | iteration-plan` — cerimonie scrum

Regole che prevalgono su tutto il resto: lavora solo in git worktree, parti da `develop`, mai push
o merge su `main`, mai leggere file `.env`, rete/disco/chiave API solo nel backend (il frontend
non vede la chiave e non parla con le fonti), nessuna nuova dipendenza senza richiesta esplicita.
Un controllo di privacy fallito blocca il push anche quando build e test sono tutti verdi.
