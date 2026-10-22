# Protezione dei merge

Il contratto di merge per `main` e `develop` è imposto da due **ruleset** GitHub. Questo file
dice cosa impongono, perché sono due e come ricrearli. Configurazione portata da voxelheim-v2.

Nella UI stanno in **Settings → Rules → Rulesets**.

## Cosa viene imposto

| Ruleset | Regola | Chi può aggirarla |
| ------- | ------ | ----------------- |
| `PR required — unresolved review threads block merge` | `pull_request`: `required_review_thread_resolution: true`, `required_approving_review_count: 0` | **nessuno** |
| `ci-gate must pass (admin can bypass)` | `required_status_checks`: `ci-gate` | admin del repository (`bypass_mode: always`) |

Entrambi valgono per `refs/heads/develop` e `refs/heads/main`.

In pratica:

- **Un thread di review non risolto blocca il merge per tutti**, proprietario compreso. Si legge
  il finding, lo si corregge o lo si respinge con una motivazione, si risolve il thread.
- **Un `ci-gate` rosso, in corso o mancante blocca il merge, ma l'admin vede l'opzione di
  bypass** e può forzare con una conferma esplicita. Serve quando la CI non può essere verde per
  motivi esterni al diff.

## Perché due ruleset

**La lista di bypass appartiene al ruleset, non alla regola.** Con un solo ruleset le due regole
sarebbero entrambe aggirabili o nessuna delle due. L'API lo riporta per ruleset come
`current_user_can_bypass`: `never` per il primo, `always` per il secondo.

## Conseguenze da sapere

- **La regola `pull_request` blocca anche i push diretti su `develop` e `main`**, per tutti e
  senza bypass. Tutto passa da una PR.
- **`required_approving_review_count` è 0 di proposito.** GitHub non permette all'autore di
  approvare la propria PR; su un repository con un solo sviluppatore un'approvazione
  obbligatoria renderebbe ogni PR non mergiabile se non con il bypass.
- **`strict_required_status_checks_policy` è false**: i branch non devono essere allineati alla
  base. Con più PR aperte in parallelo servirebbe un nuovo push dopo ogni merge.
- **È richiesto solo `ci-gate`.** `labeler` e `review` restano fuori: DeepSeek salta i run quando
  il budget di round è finito, e richiedere `review` bloccherebbe le PR. `ci-gate` è l'unico
  check stabile e contiene la regola per branch:
  - verso basi diverse da `main`: i job selezionati dal diff e `automation` devono avere
    successo; i job non selezionati possono risultare `SKIPPED`;
  - verso `main`: tutti i job devono girare e avere successo.

## Perché `READY TO MERGE` non è ridondante

Il ruleset è un pavimento. La regola nativa di GitHub conta i **thread** di review, ma DeepSeek
può lasciare finding anche nel **corpo** della review, che non crea thread. Una review fatta solo
di quelli conta zero thread aperti e GitHub lascerebbe fare il merge.

`pr-status-json` li conta a parte come `deepseek_unread_findings`, e li considera letti con la
label datata `DEEPSEEK_REVIEW_READ`. Un umano la può applicare dopo aver letto i finding; l'AI
solo dopo aver corretto o respinto con prove ogni finding e aver pubblicato le motivazioni sulla
PR. `NO_DEEPSEEK_REVIEW` resta un'esenzione solo umana. Quindi:

- il **ruleset** garantisce che nessuno faccia merge per sbaglio sopra un thread aperto o un
  `ci-gate` rosso;
- la **label `READY TO MERGE`** garantisce in più che la review è stata letta, il round è
  finito e la PR è davvero mergiabile.

## Ricreare la configurazione

```bash
gh api repos/OWNER/REPO/rulesets -X POST --input - <<'JSON'
{
  "name": "PR required — unresolved review threads block merge",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": { "ref_name": { "include": ["refs/heads/develop", "refs/heads/main"], "exclude": [] } },
  "rules": [
    { "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": false,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": true
      }
    }
  ]
}
JSON

gh api repos/OWNER/REPO/rulesets -X POST --input - <<'JSON'
{
  "name": "ci-gate must pass (admin can bypass)",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [ { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" } ],
  "conditions": { "ref_name": { "include": ["refs/heads/develop", "refs/heads/main"], "exclude": [] } },
  "rules": [
    { "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [ { "context": "ci-gate" } ]
      }
    }
  ]
}
JSON
```

`actor_id: 5` con `actor_type: RepositoryRole` è il ruolo **admin**.

## Ordine di attivazione

Non richiedere `ci-gate` prima che abbia girato su una PR vera: un check obbligatorio mai
eseguito lascia ogni PR in "expected, waiting". L'ordine sicuro è:

1. Creare il primo ruleset (PR obbligatoria).
2. Aprire una PR verso `develop` e verificare che riporti un `ci-gate` verde.
3. Solo allora creare il secondo ruleset.

## Verificare

```bash
gh api repos/OWNER/REPO/rulesets --jq '.[] | "\(.name) → \(.enforcement)"'
gh pr view <number> --json mergeable,mergeStateStatus
```

`mergeable: MERGEABLE` insieme a `mergeStateStatus: BLOCKED` indica una regola che trattiene il
merge: la PR è pulita, è la policy a dire di no.
