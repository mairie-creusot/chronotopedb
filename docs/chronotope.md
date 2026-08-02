# Chronotope — substrat spatio-temporel à horizon scellé

**Origine.** Ce concept a été proposé dans une étude d'architecture pour
[PawChat](https://github.com/mairie-creusot/pawchat) (`docs/serveur-hyperscale-metaverse.md`
§5 dans ce dépôt), comme réponse à une question précise : quelle base de
données porterait l'état temps réel d'un métavers VR à grande échelle, si
SpacetimeDB (qui reste le système transactionnel de référence pour tout le
reste) ne suffisait plus pour les poses d'avatars haute fréquence.

**Statut dans PawChat, honnêtement.** Le document source conclut que ce
concept n'est **pas nécessaire aujourd'hui** pour PawChat (échelle réelle :
10 à quelques centaines de personnes), et ne le deviendrait légitimement
qu'au-delà d'environ 500 personnes simultanées. Ce dépôt existe pour une
raison différente : **prototyper le concept sérieusement, en isolation, pour
le tester avant qu'il ne coûte des mois** — exactement la recommandation du
document source (§6, Horizon 2) : *"un prototype qui teste H1 et H2 en
isolation — quelques centaines de lignes de Rust et un client simulé, pas un
système. Un tel prototype coûte quelques semaines et peut réfuter le concept
avant qu'il ne coûte des mois, ce qui est sa vraie valeur."*

Ce dépôt n'est donc pas un composant de production. C'est un instrument de
mesure : soit il confirme H1-H4 ci-dessous, soit il les réfute, et les deux
issues ont de la valeur.

---

## Le nom

Emprunté à Bakhtine : le chronotope désigne l'indissociabilité du temps et
de l'espace dans la représentation. C'est exactement l'unité de stockage
proposée.

## 1. La thèse — la cardinalité d'écriture décide de tout

On classe habituellement l'état d'un système par **importance** ou par
**fréquence** (« chaud »/« froid »). Les deux critères sont trompeurs : une
position est critique pour l'anti-triche, et l'inventaire d'un joueur qui
agit en boucle est chaud.

Le bon critère est ailleurs : **combien d'écrivains indépendants peuvent
muter cette donnée au même instant ?**

| Donnée | Écrivains concurrents | Conséquence formelle | Système |
|---|---:|---|---|
| Pose d'un avatar | **1** (le corps décrit) | LWW par `(entité, seq)` — treillis trivialement convergent | Chronotope |
| Transformation d'un objet tenu | **1** (le porteur, arbitré) | idem | Chronotope |
| Solde, propriété, score, revendication | **N** | exige la sérialisabilité | système transactionnel (hors périmètre de ce dépôt) |

Conséquences :

1. **Un CRDT n'est pas un choix, c'est un constat.** Cardinalité 1 ⇒ le CRDT
   dégénère en registre dernier-écrivain-gagne, convergence triviale, coût
   nul. Le fardeau habituel des CRDT vient de la concurrence d'écriture —
   supprimez-la, il ne reste que la structure.
2. **La durabilité doit être fonction du taux d'auto-invalidation.** Une pose
   à 20 Hz s'invalide elle-même en 50 ms : la persister est un contresens.
   *Une donnée dont l'espérance de vie utile est inférieure au temps de son
   propre `fsync` ne doit jamais atteindre un stockage durable.*
3. **La frontière est vérifiable mécaniquement.** « Cette donnée a-t-elle
   plus d'un écrivain possible ? » se répond en lisant le code d'écriture.

**Chronotope ne gère QUE la donnée à cardinalité 1.** Tout le reste (état à
cardinalité N) est explicitement hors périmètre — ce n'est pas un manque,
c'est la conception.

## 2. L'unité de stockage n'est pas la ligne, c'est la cellule-tick

Les bases à ligne (Postgres, Redis, les bases graphe, les bases spatiales)
partagent une hypothèse : l'unité de stockage, de mutation et de
réplication est la ligne. Un joueur qui bouge, c'est un `UPDATE` d'une
ligne. Cela produit `N × F` écritures aléatoires par seconde.

Chronotope inverse : l'unité est le couple **(cellule, tick)**.

```text
chronotope := {
  clé      : (room_id: u32, cell: u32, tick: u32)
  scellé   : bool
  entités  : [ (entity_id: u32, pose) ; k ]
}
```

Conséquences :

- Une écriture est un **ajout séquentiel contigu**, jamais une mutation en
  place à adresse aléatoire.
- Le nombre d'opérations d'écriture passe de `N × F` à
  `min(cellules_occupées, N) × F`, où chaque opération porte `k` entités.
- Un chronotope scellé est immuable : réplicable, cachable, projetable en
  mémoire, sans coordination.

**L'argument central.** L'AOI (filtrage par zone d'intérêt) et l'écriture
par trame ont des pires cas *opposés* :

| Régime | AOI par cellules | Écriture par trame |
|---|---|---|
| Joueurs dispersés (1/cellule) | excellente — filtre presque tout | neutre — 1 entité par trame |
| Joueurs agglutinés (30+/cellule) | inutile — plus rien à filtrer | **excellente** — 1 écriture au lieu de 30 |

L'agglutination est le comportement normal d'un espace social. **Chronotope
n'améliore pas l'AOI ; il couvre exactement le régime où l'AOI cesse de
fonctionner.** Ensemble, les deux mécanismes bornent le coût dans les deux
régimes. C'est la contribution la plus défendable du concept — **et H1
(ci-dessous) est le test qui la vérifie directement.**

## 3. Modèle de cohérence — la cohérence par horizon scellé

Ni forte, ni « à terme ». Un modèle intermédiaire, défini par un seul
paramètre Δ.

> **Cohérence par horizon.** Soit Δ l'horizon. Une lecture du chronotope
> `(C, T)` est **scellée** — immuable, rejouable, identique sur tous les
> nœuds — si `T < maintenant − Δ`. Elle est **spéculative** sinon.

Le sceau est un fait, pas une négociation : au temps mural `T + Δ`, le
chronotope `(C, T)` est scellé, quoi qu'il arrive. Une écriture arrivant
après le sceau est **rejetée**, jamais fusionnée.

Δ est censé réutiliser tel quel le délai d'interpolation qu'un client
temps réel doit déjà subir pour absorber la gigue réseau — le tampon de
fluidité et la fenêtre de convergence de la base deviennent le même budget,
dépensé une seule fois. C'est l'hypothèse **H2**, la plus fragile du
concept : rien ne garantit a priori que ce délai suffit à la base pour
converger sans que le client en perçoive l'effet.

Avant le sceau : CRDT (registres LWW, §1). Après le sceau : journal
immuable, donc event sourcing. **CRDT et event sourcing ne sont pas deux
options concurrentes, ce sont deux phases de la vie d'une même donnée**,
séparées par un instant précis.

## 4. Partitionnement et migration — l'entité migre sans transfert d'état

La cellule est l'unité de **placement** ; l'entité est l'unité de
**migration**.

```text
migration de E, cellule A (nœud 1) → cellule B (nœud 2), au tick T :
  nœud 1 : cesse d'inscrire E dans ses chronotopes à partir de T+1
  nœud 2 : commence à inscrire E dans ses chronotopes à partir de T+1
  routage : entity_directory[E] ← (B, nœud 2)
```

Il n'y a rien à transférer parce que **chaque trame de pose est
auto-suffisante** : elle décrit intégralement l'entité au tick T, ce n'est
pas un delta appliqué à un état antérieur. Le coût de bascule est une
entrée de table de routage — pas un transfert d'état comme chez Photon
(qui documente ce transfert comme la partie difficile de son
architecture).

**Portée honnête de cette propriété.** Elle ne tient que parce que la
donnée migrée est sans mémoire. Une entité qui accumulerait de l'état
(points de vie, inventaire, progression) exigerait un vrai transfert — c'est
exactement pour cela que ce genre d'état reste hors du périmètre de ce
dépôt, dans un système transactionnel séparé qui ne partitionne jamais.

**Hystérésis obligatoire.** Le retour d'expérience SpatialOS est explicite :
sans bande de recouvrement, une entité qui longe une frontière peut migrer
en rafale (*thrashing*). L'implémentation doit inclure une hystérésis
(rayon d'entrée > rayon de sortie, ou délai minimum entre deux migrations
de la même entité) et le prouver par un test qui simule une trajectoire le
long d'une frontière — **c'est le test H3**.

## 5. Réplication — pilotée par l'intérêt, pas par la durabilité

Chronotope ne réplique pas pour la durabilité — il n'y a rien à sauver, la
donnée est morte en ~100 ms. Il réplique pour la latence.

> Le facteur de réplication d'un chronotope est le nombre de nœuds
> hébergeant au moins un observateur dont l'AOI intersecte la cellule.

- **R varie dynamiquement**, de 0 à quelques unités, en suivant l'attention
  des joueurs.
- **R = 0 signifie que la cellule n'est pas simulée du tout.**
- **Aucun consensus n'est requis.** Un chronotope scellé est immuable ; deux
  copies d'un objet immuable sont trivialement cohérentes. Ni Raft, ni
  Paxos, ni quorum dans le chemin critique.

## 6. Hiérarchie mémoire par âge

| Étage | Contenu | Latence tolérée | Usage |
|---|---|---|---|
| **T0** | les derniers ticks (≈2Δ) | chemin chaud | boucle de simulation, diffusion |
| **T1** | la dernière minute | rejeu court, arrivée tardive, spectateur | dégradé proprement en "DRAM du même nœud, plus petit" pour ce prototype — pas de matériel CXL requis |
| **T2** | chronotopes scellés compressés | ms | replay, analyse — **hors périmètre de ce prototype** |

La politique de tiering n'a besoin d'aucune heuristique : l'âge d'un
chronotope est connu exactement, il est dans sa clé. `T < maintenant − 2Δ ⇒
descendre`. Pas de LRU, pas de compteur d'accès, pas de prédiction.

## 7. Indexation — l'adresse EST la position

Aucun index spatial (ni R-tree, ni courbe de Hilbert). La requête est trop
contrainte pour le justifier : on ne demande jamais « quelles entités sont
à moins de 30 m de (x, z) », on demande « donne-moi les cellules 11 à 33 au
tick T », ce qui se résout en arithmétique :

```text
adresse(room, cell, tick) = base[room] ⊕ (cell × ANNEAU + tick mod ANNEAU)
```

O(1), sans maintenance, sans rééquilibrage.

Une seule question a besoin d'un index : « où est l'entité E ? » — un
annuaire d'une entrée par entité. C'est le rôle de `chronotope-directory`.

> **Partage de responsabilité.** Un système externe (annuaire d'identité,
> SpacetimeDB côté PawChat) est l'annuaire *entité → lieu*. Chronotope est
> le magasin *lieu → état*.

## 8. Le temps — un tick logique par salle

Le tick est un compteur monotone par salle, pas une horloge murale. Deux
salles sont supposées causalement disjointes par construction (aucune
action, aucune observation ne les relie) — il n'existe donc aucune relation
« arrivé-avant » inter-salles à préserver, et la synchronisation d'horloge
devient un confort, jamais une condition de correction. C'est une
simplification qui ne tient QUE sous cette hypothèse de disjonction ; un
monde continu sans couture ne l'aurait pas.

## 9. Dégradation — la dilatation temporelle, par cellule

EVE Online ralentit le temps pour tout un nœud (un système solaire entier)
quand il sature, plutôt que de refuser des joueurs. Chronotope transpose
l'idée avec deux améliorations structurelles :

1. **La dégradation est locale à la cellule**, pas au nœud entier —
   l'agglomération dans un endroit ne dégrade pas les endroits calmes.
2. **La dégradation ne ralentit que la cadence de scellement**, donc la
   fidélité d'observation d'autrui — jamais la simulation locale du corps
   propre (qui reste à latence zéro, côté client). On perd de la
   fraîcheur, jamais de la fluidité.

```text
budget dépassé pour la cellule C ⇒ cadence(C) : 20 Hz → 10 Hz → 5 Hz
                                    Δ(C)       : 100 ms → 200 ms → 400 ms
```

C'est le test **H4**.

## 10. Interface

Cinq opérations. Ni transaction, ni jointure, ni requête ad hoc, ni index
secondaire, ni langage de requête.

```text
sceller(room, cell, tick) -> Chronotope            // idempotent, déclenché par l'horloge
écrire(room, cell, tick, entity, pose) -> Ack|Rejet // Rejet si déjà scellé
lire(room, [cells], tick) -> [Chronotope]           // tick < maintenant - Δ garanti
observer(room, [cells]) -> Flux<Chronotope>         // pousse les sceaux
migrer(entity, cell_dest) -> ()                     // écrit l'annuaire, ne transfère rien
```

Dans ce dépôt : les quatre premières vivent dans `chronotope-core` (trait
`ChronotopeStore`) ; `migrer` vit dans `chronotope-directory`.

---

## Hypothèses falsifiables

Un concept qui ne peut pas échouer ne vaut rien.

| # | Hypothèse | Mesure | Réfutée si |
|---|---|---|---|
| **H1** | L'écriture par trame réduit le coût d'écriture dans le régime aggloméré | ops/s et µs CPU à 40 avatars dans une cellule, contre écriture par ligne | gain < 3× |
| **H2** | La cohérence par horizon est imperceptible | étude A/B en aveugle, fluidité et réactivité perçues, Δ ∈ {60, 100, 200} ms — **hors périmètre de ce dépôt (exige des sujets humains)**, mais le dépôt doit mesurer la latence/fraîcheur objective que H2 présuppose bornée | inconfort détecté à Δ = 100 ms |
| **H3** | La migration sans transfert n'introduit pas d'artefact | taux de glitch au franchissement de frontière (duplication, perte, latence de bascule), contre une bascule d'autorité classique | artefact mesurable plus fréquent |
| **H4** | La dilatation par cellule est préférable au plafond dur | cadence effective par cellule sous charge croissante, localité de la dégradation | la dégradation dégrade des cellules non chargées, ou ne se déclenche pas avant la limite dure |

**H2 est la plus fragile et la plus importante.** Tout le concept repose sur
l'idée que le délai d'interpolation dont on ne peut pas se passer peut
aussi servir de fenêtre de cohérence. Ce dépôt ne peut pas trancher le
jugement humain de H2, mais il doit produire les mesures objectives
(latence bout-en-bout, fraîcheur, taux de rejet d'écriture tardive) sur
lesquelles un futur test A/B s'appuierait.

## Modes d'échec connus

1. **L'agglutination n'est pas résolue, elle est déplacée.** Chronotope
   rend l'écriture bon marché ; il ne rend pas bon marché le fait que
   chaque entité doive en observer 59 autres dans le même cas. Le remède
   est côté observateur (LOD réseau), hors périmètre de ce dépôt.
2. **Aucune durabilité.** Un crash de nœud perd le dernier Δ. Sans
   importance pour des poses, catastrophique si une donnée à cardinalité
   N s'y glisse — la frontière du §1 doit être imposée par revue de code,
   ce dépôt ne peut pas la faire respecter automatiquement (bien qu'un lint
   ou un test de propriété sur les types publics pourrait constituer une
   piste future).
3. **Le brassage mémoire.** Sceller à haute fréquence sur des centaines de
   cellules produit un flux d'objets immuables qui exige une allocation par
   arène et un recyclage d'anneau. Travail connu en Rust, mais du travail
   réel.
4. **Aucune requête historique par entité.** « Où était E il y a 30 s ? »
   exige de balayer les cellules. Assumé : la question posée en pratique
   est « qui était là », pas « où était-il ».
5. **Un système de plus à exploiter.** Ce compromis n'est valable qu'au
   -delà d'un seuil de charge réel — voir le document source côté PawChat
   pour le calibrage complet. Ce dépôt ne préjuge pas de ce seuil ; il
   fournit seulement les mesures qui permettent de le fixer en connaissance
   de cause.

## Ordre de construction (repris du document source, §6 Horizon 2)

1. Substrat mono-nœud, poses seulement — `chronotope-core`.
2. Sceau et hiérarchie d'âge (T0/T1 en DRAM).
3. Annuaire et migration à hystérésis, multi-nœud simulé — `chronotope-directory` + `chronotope-sim`.
4. Dilatation temporelle par cellule.
5. Réseau réel (WebTransport) et CXL pour T1 — explicitement hors périmètre
   de ce prototype ; le document source ne les recommande qu'après
   validation de H1-H4 en isolation.

## Ce que ce dépôt ne fait délibérément pas

- Pas de couche réseau réelle (pas de WebTransport, pas de QUIC) — le
  harnais de simulation (`chronotope-sim`) reste en mémoire, dans un seul
  process. Ajouter un vrai transport est un projet séparé, une fois le
  modèle de données validé.
- Pas de CXL — T1 se dégrade en "DRAM du même nœud, plus petit" comme
  recommandé par le document source pour un prototype.
- Pas de persistance T2 (stockage objet, replay) — hors périmètre de
  H1-H4.
- Pas de gestion de la donnée à cardinalité N (inventaire, économie,
  scores) — c'est explicitement le rôle d'un autre système.
