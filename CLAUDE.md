# CLAUDE.md

Leggi e segui le istruzioni del file @AGENTS.md alla radice del progetto.

Il repository è privato, ma niente segreti nel codice: la chiave CurseForge sta solo in `.env`
(ignorato da git) e nel backend Rust. Gli agenti non leggono mai `.env`; per sapere quali
variabili servono usano `.env.example`. Niente firme degli strumenti in commit e PR: nessun
trailer `Co-Authored-By` di agenti, nessun "Generated with", nessun link `claude.ai/code/session_…`.

Riferimento rapido — le skill della pipeline:

- `/dev-issue <numero>` — implementa una issue GitHub dall'inizio alla fine (worktree → gate → PR)
- `/process-pr [numero]` — ciclo forzato su una PR aperta (feedback DeepSeek + correzioni CI)
- `/scrum-master feature-spec | backlog-refine | iteration-plan` — cerimonie scrum

Regole che prevalgono su tutto il resto: lavora solo in git worktree, parti da `develop`, mai push
o merge su `main`, mai leggere file `.env`, rete/disco/chiave API solo nel backend (il frontend
non vede la chiave e non parla con le fonti), nessuna nuova dipendenza senza richiesta esplicita.
