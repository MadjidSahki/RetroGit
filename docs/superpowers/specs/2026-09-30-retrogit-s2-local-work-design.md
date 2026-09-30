# RetroGit — Sous-projet 2 : Travail local (status, diff, staging, commit)

- **Date** : 2026-09-30
- **Statut** : design validé en conversation, en attente de revue de la spec écrite
- **Prérequis** : sous-projet 1 terminé et fusionné dans `main`
  (`docs/superpowers/specs/2026-09-30-retrogit-s1-foundation-github-design.md`)

## 1. Objectif

Permettre le travail local quotidien dans un dépôt ouvert : voir ce qui a changé, lire les
diffs, choisir précisément ce qui part dans le prochain commit (fichier, hunk ou ligne),
committer (avec hooks et signature) et corriger le dernier commit.

### Critères de succès

1. Le panneau central d'un dépôt ouvert affiche deux groupes, « Staged changes » et
   « Changes » (fichiers non suivis compris). Le status se met à jour seul quand des
   fichiers changent sur disque.
2. Cliquer sur un fichier affiche son diff unifié (côté staged ou unstaged).
3. On peut stager et déstager un fichier entier, un hunk ou une sélection de lignes. Le
   résultat est identique à ce que produirait `git add -p` / `git reset -p` avec la même
   sélection.
4. On peut committer les changements stagés. Les hooks (`pre-commit`, `commit-msg`…) et la
   signature configurée s'exécutent, car le commit passe par le `git` installé.
5. On peut amender le dernier commit (message et/ou contenu).
6. On peut ajouter un fichier ou une extension au `.gitignore`.
7. L'interface ne bloque jamais, y compris pendant un hook lent ; CPU ≈ 0 % au repos.
8. Fonctionne sur macOS arm64 et Windows x86_64.

### Hors périmètre

- Discard des modifications (annuler des changements du working tree).
- Résolution des conflits de merge : les fichiers en conflit sont affichés, pas stageables.
- Stash, historique, branches, fetch/pull/push (sous-projet 3), Pull Requests (sous-projet 4).
- Staging partiel des fichiers binaires (fichier entier uniquement).

## 2. Décisions techniques

| Sujet | Choix | Raison |
|---|---|---|
| Status, diff, staging | `git2` | Rapide, en processus, déjà utilisé |
| Staging partiel | Calcul du nouveau contenu de l'index par une fonction pure (`apply_selection`), puis écriture d'un blob et mise à jour de l'entrée d'index | Même principe que `git add -p`, sans générer ni analyser de patch texte ; entièrement testable |
| Commit | `git commit` du CLI, repli `git2` si `git` est introuvable | Hooks, signature GPG/SSH et config globale respectés |
| Surveillance du disque | crate `notify` (FSEvents / ReadDirectoryChangesW), événements regroupés sur 300 ms | Natif, léger, multi-plateforme |
| Nouveaux widgets | `win95::checkbox`, `win95::text_area` | Génériques, réutilisables |

## 3. Architecture

Aucune nouvelle crate. Les règles de dépendance du sous-projet 1 restent : `win95`,
`gitcore` et `github` ne dépendent pas les uns des autres, et aucun type `git2` n'apparaît
dans l'API publique de `gitcore`.

### 3.1 `gitcore`

- `status.rs`
  - `Repo::status() -> Result<Vec<FileStatus>, GitError>`
  - `FileStatus { path: String /* relatif, séparateur '/' */, staged: Option<Change>, unstaged: Option<Change> }`
  - `Change::{Added, Modified, Deleted, Renamed { from: String }, TypeChange, Untracked, Conflicted}`
  - Fichiers ignorés exclus ; fichiers non suivis inclus (répertoires non suivis parcourus).
  - Tri par chemin.
- `diff.rs`
  - `Repo::diff_file(path: &str, side: Side) -> Result<FileDiff, GitError>`, `Side::{Unstaged, Staged}`
    (unstaged = index → working tree ; staged = HEAD → index, ou arbre vide si pas de HEAD).
  - `FileDiff { path, binary: bool, hunks: Vec<Hunk> }`
  - `Hunk { header: String, old_start, old_lines, new_start, new_lines: u32, lines: Vec<DiffLine> }`
  - `DiffLine { kind: LineKind::{Context, Added, Removed}, old_no: Option<u32>, new_no: Option<u32>, text: String, no_newline_at_eof: bool }`
  - Un fichier non suivi donne un diff « tout ajouté » depuis un contenu vide.
  - Le diff de libgit2 est calculé sur le contenu **filtré** du working tree (conversion
    CRLF → LF selon `core.autocrlf` / `.gitattributes`), c'est-à-dire ce que Git indexerait.
- `stage.rs`
  - `Selection::{All, Hunks(Vec<usize>), Lines(Vec<(usize /*hunk*/, usize /*line*/)>)}`
  - Fonction pure :
    `apply_selection(base: &[u8], diff: &FileDiff, selection: &Selection, direction: Direction) -> Result<Vec<u8>, GitError>`,
    `Direction::{Stage, Unstage}`.
    - **Stage** : `base` = contenu de l'index (vide si le fichier n'y est pas),
      `diff` = diff unstaged. On parcourt `base` : les lignes de contexte sont gardées, un
      `-` sélectionné est retiré (non sélectionné : gardé), un `+` sélectionné est inséré
      (non sélectionné : ignoré).
    - **Unstage** : `base` = contenu de l'index, `diff` = diff staged (HEAD → index). Même
      parcours sur le côté « nouveau » : un `+` sélectionné est retiré (non sélectionné :
      gardé), un `-` sélectionné est réinséré (non sélectionné : ignoré).
    - Le marqueur de fin de fichier sans retour à la ligne est respecté dans les deux sens.
  - `Repo::stage(path, &Selection)` / `Repo::unstage(path, &Selection)`
    - `All` : ajout ou retrait direct dans l'index (`add_path`, `remove_path`, ou
      restauration de l'entrée HEAD pour l'unstage).
    - Partiel : recalcul du diff, vérification que la sélection correspond toujours
      (sinon `GitError::StaleSelection`), `apply_selection`, écriture du blob, mise à jour
      de l'entrée d'index en gardant son mode.
    - Refus des sélections partielles pour les binaires, les suppressions et les conflits
      (`GitError::Unsupported(String)`).
- `commit.rs`
  - `Repo::commit(message: &str, amend: bool) -> Result<CommitOutcome, GitError>`
  - `CommitOutcome { commit: CommitInfo, used_cli: bool }`
  - CLI : `git -C <repo> commit -F - [--amend]`, message sur stdin, variables
    `GIT_TERMINAL_PROMPT=0` et `GIT_EDITOR=true`, stdout/stderr capturés.
    - Code ≠ 0 → `GitError::CommitRejected { output: String }` (sortie des hooks ou de la
      signature, tronquée à 20 000 caractères).
  - Repli `git2` si l'exécutable `git` est introuvable : commit avec la signature
    `user.name` / `user.email` de la config (sinon `GitError::MissingIdentity`),
    `used_cli = false`.
  - `Repo::last_commit_message() -> Result<Option<String>, GitError>` (pré-remplissage de l'amend).
  - `Repo::head_is_pushed() -> Result<bool, GitError>` : vrai si la branche a un upstream
    qui contient déjà HEAD (avertissement d'amend).
- `ignore.rs`
  - `Repo::add_to_gitignore(pattern: &str) -> Result<(), GitError>` : ajoute une ligne au
    `.gitignore` racine (création si besoin, pas de doublon, retour à la ligne final garanti).

Nouvelles variantes de `GitError` : `StaleSelection`, `Unsupported(String)`,
`CommitRejected { output: String }`, `MissingIdentity`.

### 3.2 `win95`

- `checkbox(ui, checked: &mut bool, label: &str) -> Response` : case 13×13 blanche enfoncée
  (bevel `Field`), coche noire, libellé à droite, accessible (`WidgetType::Checkbox`).
- `text_area(ui, text: &mut String, width: f32, rows: usize) -> Response` : champ
  multiligne blanc enfoncé.

### 3.3 `app`

- `watch.rs` : `Watcher::start(repo_path, notify: impl Fn() + Send) -> Result<Watcher, _>`.
  - Surveillance récursive. Ignore tout ce qui est sous `.git/`, sauf `.git/index` et
    `.git/HEAD`.
  - Regroupement sur 300 ms : un seul appel à `notify` par rafale.
  - Remplacé à chaque ouverture de dépôt ; arrêté à la fermeture de l'app.
- Worker : nouvelles commandes `RefreshStatus`, `LoadDiff { path, side }`,
  `Stage { path, selection }`, `Unstage { path, selection }`,
  `Commit { message, amend }`, `AddToGitignore(pattern)`, `LoadLastCommitMessage`.
  - Toutes s'appliquent au dépôt courant, que le worker mémorise à chaque `RepoOpened` /
    `CloneDone`.
  - Au plus un `RefreshStatus` en attente dans la file : les demandes en double sont
    ignorées (drapeau atomique partagé avec le handle).
- Nouveaux événements : `StatusLoaded(Vec<FileStatus>)`, `DiffLoaded(FileDiff)`,
  `Committed(CommitOutcome)`, `LastCommitMessage(Option<String>)`, `HeadPushed(bool)`.
- État : `ChangesView { status, selected_file: Option<(String, Side)>, diff: Option<FileDiff>,
  selected_lines: BTreeSet<(usize, usize)>, summary, description, amend, committing: bool }`.
  - `selected_lines` est vidé à chaque nouveau `DiffLoaded`.

## 4. Écran « Changes »

Le panneau central d'un dépôt ouvert devient :

```
┌ acme-cor-lab · main · a1b2c3d "fix …" ───────────────── [Refresh] ┐
│ Staged changes (2)   [Unstage all] │ src/lib.rs (unstaged)         │
│  [M] src/lib.rs                    │ [Stage selected lines]        │
│  [A] README.md                     │ @@ -10,4 +10,6 @@ [Stage hunk]│
│ Changes (3)            [Stage all] │ [ ] 10 10   fn a() {          │
│  [M] Cargo.toml                    │ [x]    11 +     new line      │
│  [?] notes.txt                     │ [ ] 11    -     old line      │
├────────────────────────────────────┴───────────────────────────────┤
│ Summary: [________________________]      [ ] Amend last commit     │
│ Description: [                                        ]  [Commit]  │
└────────────────────────────────────────────────────────────────────┘
```

- **Liste des fichiers** : icônes `[M]` modifié, `[A]` ajouté, `[D]` supprimé,
  `[R]` renommé (`old → new`), `[T]` changement de type, `[?]` non suivi, `[!]` conflit.
  - Un fichier partiellement stagé apparaît dans les deux groupes.
  - Clic : affiche le diff du côté correspondant.
  - Double-clic ou Espace : stage / unstage du fichier entier.
  - Clic droit : Stage, Unstage, « Add to .gitignore » (chemin exact ou `*.ext`).
- **Diff** : unifié, numéros avant/après, fond vert pâle pour `+`, rouge pâle pour `-`,
  police W95FA (ou monospace libre si l'alignement est mauvais).
  - Case à cocher sur chaque ligne `+` / `-`. Bouton « Stage selected lines » (ou
    « Unstage selected lines ») actif quand au moins une ligne est cochée.
  - Bouton « Stage hunk » (ou « Unstage hunk ») sur chaque en-tête de hunk.
  - Binaire : « Binary file » et bouton « Stage file » / « Unstage file ».
  - Virtualisé ; au-delà de 20 000 lignes, « Diff too large » et bouton « Show anyway ».
  - Conflit : « Resolve conflicts first » (aucun bouton de staging).
- **Zone de commit** :
  - [Commit] actif si (au moins un changement stagé **ou** amend coché) et Summary non vide.
  - Message = Summary, puis ligne vide et Description si elle n'est pas vide.
  - Cocher « Amend » pré-remplit Summary/Description avec le dernier message (si les champs
    sont vides) et affiche un avertissement si HEAD est déjà poussé.
  - Pendant le commit : champs et bouton désactivés, barre de progression marquee
    « Committing… ».
  - Après succès : champs vidés, amend décoché, status et résumé du dépôt rafraîchis,
    barre d'état « Committed a1b2c3d ». Un repli git2 ajoute un avertissement unique
    « git not found: hooks and signing were skipped ».
- **Rafraîchissement** : à l'ouverture du dépôt, après chaque opération, sur signal du
  watcher, au retour du focus de la fenêtre et via [Refresh]. Si le fichier affiché a
  changé, son diff est rechargé.
- **Menu Repository** : « Commit… » (focus sur Summary), « Stage all », « Unstage all ».

## 5. Flux de données

1. Une action de l'utilisateur envoie une `Command` au worker.
2. Le worker exécute l'opération sur le dépôt courant, puis renvoie systématiquement un
   `StatusLoaded` complet, et un `DiffLoaded` si un fichier est affiché.
3. Le reducer `apply` met à jour `ChangesView` et vide la sélection de lignes à chaque
   nouveau diff.
4. Le watcher appelle `notify`, qui envoie `RefreshStatus` (dédoublonné) et réveille egui.

## 6. Erreurs et cas limites

| Situation | Comportement |
|---|---|
| Sélection périmée (fichier modifié entre affichage et clic) | Rien n'est écrit ; diff rechargé ; message « File changed, diff reloaded ». |
| Fin de fichier sans retour à la ligne | Conservée ; l'ajout ou le retrait du retour final est stageable seul. |
| CRLF / `core.autocrlf` | Pas de faux diff : le diff porte sur le contenu filtré, le contenu de l'index est construit à partir du blob d'index. |
| Fichier non suivi stagé partiellement | Base vide ; seules les lignes choisies entrent dans l'index. |
| Suppression, binaire | Fichier entier uniquement. |
| Rename | Affiché `[R] old → new` ; fichier entier uniquement : stager/déstager applique ensemble la suppression de `old` et l'ajout de `new`. |
| Conflit | Affiché `[!]`, non stageable : « Resolve conflicts first ». |
| `user.name` / `user.email` absents | Message avec les commandes `git config --global user.name …` / `user.email …`. |
| Hook ou signature qui échoue | Message d'erreur avec la sortie (zone défilable) ; message de commit conservé. |
| `git` introuvable | Commit via git2 ; avertissement unique (hooks et signature ignorés). |
| Amend d'un commit déjà poussé | Autorisé, avec avertissement visible sous la case Amend. |
| Rafale d'événements disque | Au plus un `RefreshStatus` en attente. |
| Watcher impossible à démarrer | Log d'avertissement ; rafraîchissement au focus et via [Refresh]. |

## 7. Tests

- **`apply_selection`** (unitaires, purs) : ajout, suppression et modification d'une ligne ;
  plusieurs hunks dont un seul sélectionné ; lignes non contiguës ; fin de fichier sans
  retour à la ligne (dans les deux sens) ; fichier vide ; fichier nouveau ; contenu CRLF ;
  réciprocité (stage puis unstage de la même sélection redonne l'index initial) ;
  équivalence avec `git apply --cached` sur un jeu de cas (test ignoré si `git` absent).
- **`gitcore`** (intégration, dépôts temporaires) : status pour chaque type de changement ;
  diff unstaged / staged / non suivi / binaire ; stage et unstage (fichier, hunk, lignes) ;
  sélection périmée ; commit via CLI ; commit refusé par un hook `pre-commit` (sortie
  capturée) ; repli git2 ; identité manquante ; amend ; `last_commit_message` ;
  `head_is_pushed` ; `add_to_gitignore` (création, doublon, retour à la ligne final).
  Les tests qui exigent `git` sont ignorés s'il est absent.
- **`app`** : reducer pour les nouveaux événements (dont la purge de la sélection et la
  règle d'activation du bouton Commit) ; worker : ouvrir → stage → status → commit ;
  dédoublonnage de `RefreshStatus`.
- **`win95`** : kittest pour `checkbox` (clic, accessibilité) et `text_area` (saisie).
- **Watcher** : écrire un fichier dans un dépôt temporaire → signal en moins de 2 s ;
  une écriture dans `.git/objects` seule ne déclenche rien.

## 8. Build

- Nouvelle dépendance : `notify` (dernière version stable, features par défaut), déclarée
  dans le workspace, utilisée par `app` uniquement.
- Aucun changement de profil ; le binaire release doit rester sous 15 Mo.
