# ChronotopeDB

Prototype de recherche : un substrat de stockage spatio-temporel pour l'état
temps réel à cardinalité d'écriture 1 (poses d'avatars, transformations
d'objets tenus) dans un métavers persistant. Complète un système
transactionnel classique (économie, inventaire, propriété) sans le
remplacer — il ne gère jamais de donnée à plusieurs écrivains concurrents.

Conçu à l'origine pour [PawChat](https://github.com/mairie-creusot/pawchat),
qui utilise [SpacetimeDB](https://spacetimedb.com) comme système
transactionnel de référence. **Ce dépôt ne remplace pas SpacetimeDB** — voir
`docs/chronotope.md` pour l'arbitrage exact (§1, la frontière par
cardinalité d'écriture) et pour une évaluation honnête de la maturité du
concept (statut : recherche, non prêt pour la production).

## L'idée en une phrase

L'unité de stockage n'est pas la ligne, c'est le couple **(cellule
spatiale, tick logique)** : un chronotope contient l'état empaqueté de
toutes les entités d'une cellule à un instant, ce qui transforme des
écritures aléatoires en ajouts séquentiels contigus — et bascule, un délai
Δ après son tick, d'un CRDT dégénéré (écrivain unique) vers un journal
immuable.

Voir [`docs/chronotope.md`](docs/chronotope.md) pour le concept complet,
avec ses hypothèses falsifiables (H1-H4) et ses modes d'échec connus —
lecture nécessaire avant de toucher au code.

## Structure du workspace

| Crate | Rôle |
|---|---|
| [`chronotope-core`](crates/chronotope-core) | Le substrat lui-même : `ChronotopeStore` (trait), `ChronotopeEngine` (l'implémentation cellule-tick), `NaiveRowStore` (témoin de comparaison pour H1). |
| [`chronotope-directory`](crates/chronotope-directory) | Annuaire entité → (nœud, cellule), migration sans transfert d'état, hystérésis. |
| [`chronotope-sim`](crates/chronotope-sim) | Harnais de simulation multi-nœud en mémoire, pour tester H3 (migration) et H4 (dégradation sous charge) sans réseau réel. |

## État du projet

Prototype actif — voir les hypothèses H1-H4 dans `docs/chronotope.md` pour
ce qui est en cours de validation. Aucune garantie de stabilité d'API ;
c'est un instrument de mesure, pas une bibliothèque de production.

## Développement

```bash
cargo test --workspace          # tests unitaires + d'intégration
cargo bench -p chronotope-core  # H1 : écriture par trame contre écriture par ligne
```

## Licence

Apache-2.0 — voir [LICENSE](LICENSE).
