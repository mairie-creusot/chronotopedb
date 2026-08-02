# Conventions d'ingénierie

Ce document fixe le niveau de rigueur attendu dans ce dépôt — sécurité,
modularité, maintenabilité, observabilité — au même niveau que le fork
SpacetimeDB qui backe PawChat (`mairie-creusot/SpacetimeDB`). Tout code
ajouté à ce dépôt doit s'y conformer ; une revue qui ne le vérifie pas n'est
pas une revue.

## Sécurité

- **`unsafe` n'est pas interdit, mais il est exceptionnel.** Le mode
  d'échec §5.11.3 de `docs/chronotope.md` (brassage mémoire — allocation
  par arène, recyclage d'anneau) peut légitimement en avoir besoin pour la
  performance. Toute utilisation :
  - est isolée dans le plus petit module possible, jamais dispersée ;
  - porte un commentaire `// SAFETY: ...` juste au-dessus expliquant
    précisément quel invariant la rend correcte (pas "c'est plus rapide") ;
  - est couverte par un test qui échouerait si l'invariant était violé
    (Miri quand c'est pertinent : `cargo miri test -p chronotope-core`).
- **`#![deny(unsafe_code)]` au niveau du crate par défaut**, levé
  explicitement (`#[allow(unsafe_code)]` local, jamais un blanket au niveau
  du fichier) uniquement là où c'est justifié comme ci-dessus.
- **Aucune donnée non fiable n'atteint le moteur sans validation à la
  frontière.** `chronotope-server` est la seule frontière de confiance de ce
  dépôt (c'est lui qui reçoit du JSON depuis l'extérieur) : il valide et
  renvoie une erreur HTTP propre, il ne laisse jamais une entrée malformée
  provoquer un panic dans `chronotope-core`. `chronotope-core` lui-même,
  utilisé en bibliothèque, peut légitimement `panic!` sur une violation
  d'invariant appelant (ex. cellule hors domaine) — c'est un bug de
  l'appelant, pas une entrée réseau.
- **`cargo audit` est un gate CI**, pas un rapport consultatif (voir
  `.github/workflows/ci.yml`) — toute vulnérabilité connue dans l'arbre de
  dépendances fait échouer la CI.
- **Pas de secret en dur, jamais.** Rien dans ce dépôt n'en a besoin
  aujourd'hui (pas d'auth, pas de réseau externe) ; si un jour `migrer`
  doit être exposé sur un réseau non fiable, l'authentification devient une
  frontière à part entière, avec le même traitement que `chronotope-server`
  aujourd'hui.

## Modularité

- **La frontière de crate suit la frontière de responsabilité**, pas la
  commodité du moment : `chronotope-core` ne connaît ni les nœuds ni le
  réseau ; `chronotope-directory` ne connaît pas la structure interne d'un
  chronotope, seulement le trait `ChronotopeStore` ; `chronotope-sim` ne
  connaît que les deux premiers, jamais l'inverse. Une dépendance qui
  remonte ce sens est un défaut d'architecture à corriger, pas un détail.
- **Le contrat public (`ChronotopeStore`, les types de `lib.rs`) est figé
  délibérément.** Le faire évoluer est possible mais doit être une décision
  explicite documentée dans le message de commit — pas un effet de bord
  d'une implémentation qui aurait été plus simple avec une signature
  différente.
- **Un module, une responsabilité lisible en une phrase.** Si le rôle d'un
  fichier ne tient pas dans le commentaire de module en tête (voir les
  fichiers déjà en place pour le format), il fait probablement deux choses
  et doit être scindé.

## Maintenabilité

- **`cargo fmt` et `cargo clippy --all-targets -- -D warnings` sont des
  gates CI**, jamais des suggestions. Un warning clippy qui persiste est
  soit corrigé, soit explicitement justifié par un `#[allow(...)]` local
  avec un commentaire disant pourquoi.
- **Toute fonction publique porte une doc rustdoc** expliquant le POURQUOI
  quand il n'est pas évident (le contrat, les invariants, le renvoi vers
  `docs/chronotope.md` pour le raisonnement de fond) — pas une paraphrase du
  nom de la fonction.
- **`todo!()` est un état de développement légitime**, jamais un état de
  merge. Une PR qui laisse un `todo!()` sur un chemin que ses propres tests
  exercent n'est pas prête.
- **Les hypothèses falsifiables (H1-H4, `docs/chronotope.md`) sont les
  tests qui comptent le plus.** Un changement qui fait passer H1 sous le
  seuil de 3× (ou qui rend H3/H4 impossibles à mesurer) est une régression
  même si tous les autres tests passent.

## Logs et traçabilité

Objectif explicite : pouvoir déboguer un comportement en production (ou en
simulation `chronotope-sim`) en lisant les logs seuls, sans ressortir un
débogueur.

- **`tracing`, jamais `println!`/`eprintln!`.** Un seul export contrôlé
  (`RUST_LOG`), jamais un mélange de canaux de sortie.
- **Champs structurés, pas d'interpolation de chaîne.** `tracing::info!(room
  = room.0, cell = cell.0, tick = tick.0, "chronotope scellé")`, jamais
  `tracing::info!("chronotope scellé pour room {room:?}")`. Un champ
  structuré est filtrable et agrégeable ; une chaîne interpolée ne l'est
  pas.
- **`#[tracing::instrument]` sur chaque méthode publique de
  `ChronotopeStore` et `Directory`**, avec les champs clés de son entrée
  (room, cell, tick, entity selon pertinence) — chaque opération doit
  produire un empan (span) qu'on peut suivre de bout en bout dans
  `chronotope-sim` comme dans `chronotope-server`.
- **Niveaux disciplinés** : `trace` pour le détail d'implémentation
  (chaque écriture individuelle), `debug` pour les décisions (une migration
  déclenchée, un scellement), `info` pour les événements notables au
  démarrage/arrêt, `warn` pour un état dégradé mais géré (canal
  d'observation plein, cadence dégradée §5.8), `error` réservé aux
  situations qui exigent une action humaine. Si tout est en `info`,
  `RUST_LOG` ne sert à rien.
- **`RUST_LOG` doit rester configurable à l'exécution** (déjà en place dans
  `chronotope-server`, voir `main.rs`) — jamais un niveau figé en dur dans
  le code.

## Tests

- Tests unitaires dans chaque crate, exerçant le contrat de
  `ChronotopeStore` de façon identique pour `ChronotopeEngine` ET
  `NaiveRowStore` (voir le commentaire de `store.rs`) — une suite de tests
  partagée paramétrée par l'implémentation est préférable à deux suites
  dupliquées qui peuvent diverger silencieusement.
- Tests de propriété (`proptest`, déjà en dépendance de dev) pour les
  invariants qui doivent tenir sur un grand espace d'entrées : l'ordre des
  entités dans un `Chronotope` reste trié, un chronotope scellé refuse
  toute écriture ultérieure quel que soit l'ordre d'arrivée, une migration
  ne duplique ni ne perd jamais une entité, etc.
- Tests d'intégration multi-nœuds dans `chronotope-sim` pour H3/H4 — voir
  le module doc de ce crate.
- Benchmarks `criterion` dans `chronotope-core/benches` pour H1 — voir le
  fichier déjà en place et son commentaire.
