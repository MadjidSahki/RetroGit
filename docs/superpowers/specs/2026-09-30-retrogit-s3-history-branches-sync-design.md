# RetroGit — Sous-projet 3 : Historique, branches, synchronisation

- **Date** : 2026-09-30
- **Statut** : design validé en conversation, en attente de revue de la spec écrite
- **Prérequis** : sous-projets 1 et 2 fusionnés dans `main`

## 1. Objectif

Travailler au quotidien avec l'historique et le remote depuis RetroGit : voir le graphe des
commits et le détail d'un commit, gérer les branches, et synchroniser avec GitHub (fetch,
pull, push). Tous les commits créés par RetroGit (commit, merge, rebase) respectent la
signature configurée : les dépôts ExampleOrg exigent des commits signés (GPG).

### Critères de succès

1. Un onglet « History » affiche le graphe des commits de toutes les branches (couloirs
   colorés, étiquettes de refs) et reste fluide sur 10 000 commits.
2. Cliquer sur un commit affiche son message complet, son auteur, sa date, son statut de
   signature, ses fichiers modifiés et le diff de chaque fichier (lecture seule).
3. On peut lister, créer, basculer, renommer et supprimer des branches locales, basculer sur
   une branche distante (création de la branche de suivi) et publier une nouvelle branche.
4. Fetch, pull et push fonctionnent sur les dépôts github.com en HTTPS (token RetroGit) et
   en SSH (agent de l'utilisateur), sans jamais bloquer sur une invite de mot de passe.
5. Un pull divergent demande Merge ou Rebase ; un push refusé propose de faire un pull.
6. Changer de branche avec des modifications : elles suivent si Git le permet, sinon
   RetroGit propose stash → bascule → réapplication.
7. Aucun commit non signé n'est créé quand `commit.gpgsign=true` ; l'écran de commit dit
   clairement si le prochain commit sera signé.
8. L'interface ne bloque jamais ; toute opération réseau est annulable ; CPU ≈ 0 % au repos.
9. macOS arm64 et Windows x86_64.

### Hors périmètre

Tags, cherry-pick, revert, rebase interactif, gestion des remotes (ajout, suppression,
renommage), outil de résolution de conflits, fetch automatique périodique, Pull Requests
(sous-projet 4), plusieurs comptes GitHub.

## 2. Décisions techniques

| Sujet | Choix | Raison |
|---|---|---|
| Graphe | Calcul des couloirs par une fonction pure sur un parcours git2 | Rendu propre, rapide, testable |
| Historique, branches (lecture) | `git2` | Rapide, en processus |
| Opérations qui écrivent (switch, create, rename, delete, stash, merge, rebase, abort) | `git` CLI | Hooks, signature, comportement identique au terminal |
| Fetch, pull, push | `git` CLI | SSH et identifiants de l'utilisateur ; git2 est compilé sans SSH |
| Identifiants github.com HTTPS | Le binaire RetroGit sert de `GIT_ASKPASS` et fournit `x-access-token` + le token | Fonctionne sans configuration, sans écrire le token sur disque |
| Statut de signature | `git log -1 --format=%G?` uniquement dans le détail d'un commit | La vérification lance gpg : trop lente pour toute la liste |
| Pull | `--ff-only` d'abord ; si divergence, choix Merge / Rebase | Décision validée |

Si le `git` CLI est absent, les fonctions de ce sous-projet qui en dépendent sont désactivées
avec le message « Install Git to use this feature » ; l'historique et la liste des branches
restent disponibles (git2).

## 3. Architecture

Aucune nouvelle crate. Règles inchangées : pas de dépendance entre `win95`, `gitcore` et
`github` ; aucun type `git2` dans l'API publique de `gitcore`.

### 3.1 `gitcore`

- `log.rs`
  - `LogEntry { id: String, short_id: String, parents: Vec<String>, author: String, email: String, time: i64, summary: String, refs: Vec<RefLabel> }`
  - `RefLabel { name: String, kind: RefKind::{Head, LocalBranch, RemoteBranch, Tag} }`
  - `Repo::log(skip: usize, limit: usize) -> Result<Vec<LogEntry>, GitError>` : toutes les
    branches locales et distantes plus HEAD, ordre topologique puis chronologique, pages de
    500.
- `graph.rs` (pur)
  - `GraphRow { column: usize, color: usize, edges: Vec<Edge> }`,
    `Edge { from_col: usize, to_col: usize, color: usize }` (segments vers la ligne suivante).
  - `layout(entries: &[LogEntry]) -> Vec<GraphRow>`, et une variante incrémentale qui
    reprend l'état des couloirs pour les pages suivantes.
  - Règles : un commit prend le couloir qui l'attendait (sinon le premier libre) ; son
    premier parent hérite du couloir, les autres parents ouvrent ou rejoignent un couloir ;
    un couloir libéré est réutilisé ; les couleurs suivent le couloir (palette de 8).
- `commit_detail.rs`
  - `CommitDetail { entry: LogEntry, message: String, committer: String, files: Vec<FileStatus> }`
  - `Repo::commit_detail(id) -> Result<CommitDetail, GitError>` ; diff d'un fichier contre le
    premier parent via `Repo::commit_file_diff(id, path) -> Result<FileDiff, GitError>`.
  - `Repo::signature_status(id) -> Result<SignatureStatus, GitError>` :
    `SignatureStatus::{Good { signer: String }, Bad, Unknown, Unsigned}` (d'après `%G?` et `%GS`).
- `branch.rs`
  - `Branch { name: String, remote: bool, is_head: bool, upstream: Option<String>, ahead: usize, behind: usize }`
  - `Repo::branches() -> Result<Vec<Branch>, GitError>` (git2).
  - `create_branch(name, switch)`, `switch_branch(name)`, `rename_branch(old, new)`,
    `delete_branch(name, force)`, `checkout_remote_branch("origin/x")` (crée `x` qui suit
    `origin/x`), toutes via le CLI.
  - `validate_branch_name(name) -> Result<(), String>` (`git check-ref-format --branch`).
  - Erreurs dédiées : `GitError::WouldOverwrite { files }` (bascule impossible à cause de
    modifications locales), `GitError::NotMerged(String)` (suppression d'une branche non
    fusionnée sans force).
- `remote.rs`
  - `NetAuth { github_token: Option<String> }` et `NetProgress { phase: String, percent: Option<u8> }`.
  - `Repo::fetch(&NetAuth, progress, cancel) -> Result<(), GitError>` (`fetch --prune --progress origin`).
  - `Repo::pull(&NetAuth, PullMode::{FastForwardOnly, Merge, Rebase}, progress, cancel) -> Result<PullOutcome, GitError>` ;
    `PullOutcome::{UpToDate, FastForwarded, Merged, Rebased, Conflicts}` ;
    `GitError::Diverged { ahead, behind }` en mode `FastForwardOnly`.
  - `Repo::push(&NetAuth, PushMode::{Normal, SetUpstream, ForceWithLease}, progress, cancel) -> Result<(), GitError>` ;
    `GitError::PushRejected` (non fast-forward).
  - Progression lue sur stderr de `git --progress` (lignes séparées par `\r`), fonction pure
    `parse_progress(line) -> Option<NetProgress>`.
  - Annulation : le processus `git` est tué ; un fetch annulé ne laisse rien de cassé (Git
    le garantit) ; un pull annulé pendant un merge ou rebase laisse l'état « en cours »,
    signalé par le bandeau (§4).
- `net.rs` : construction de la commande réseau.
  - Toujours : `GIT_TERMINAL_PROMPT=0`, `GIT_SSH_COMMAND="ssh -o BatchMode=yes"`,
    `--progress`, le PATH de `set_git_search_path`.
  - Si l'URL de `origin` commence par `https://github.com/` et qu'un token est fourni :
    `-c credential.helper=` (désactive les helpers pour cet appel),
    `GIT_ASKPASS=<chemin de l'exécutable RetroGit>`, `RETROGIT_ASKPASS_TOKEN=<token>`
    (uniquement dans l'environnement de ce processus).
  - `askpass_answer(prompt: &str, token: &str) -> String` (pur) : `x-access-token` si
    l'invite contient « Username », sinon le token.
- `stash.rs` : `stash_push(message) -> Result<bool /*quelque chose a été mis de côté*/, GitError>`,
  `stash_pop() -> Result<(), GitError>` (`GitError::StashConflict` si la réapplication crée
  des conflits ; le stash est alors conservé).
- `ops.rs` : `Repo::operation_in_progress() -> Option<Operation::{Merge, Rebase}>`,
  `abort_operation()`, `continue_rebase()` (CLI, `GIT_EDITOR=true`).
- `signing.rs` : `SigningConfig { enabled: bool, format: SigningFormat::{Gpg, Ssh, X509}, key: Option<String> }`,
  `Repo::signing_config()` (config effective, `includeIf` compris, lue via git2).
- **Modification du sous-projet 2** : `Repo::commit` refuse le repli git2 avec
  `GitError::SigningRequiresGit` quand `commit.gpgsign=true`.

### 3.2 `win95`

- `splitter(ui, id, fraction: &mut f32, add_top, add_bottom)` : séparation horizontale
  déplaçable (bevel Raised de 4 px).
- `combo_box(ui, id, selected_text, add_items) -> Response` : liste déroulante Win95
  (champ enfoncé blanc + bouton flèche).

### 3.3 `app`

- `main.rs` : si le premier argument est `--askpass`, lit `RETROGIT_ASKPASS_TOKEN` et la
  question (argument suivant), affiche `askpass_answer` sur stdout et quitte avant toute
  initialisation graphique.
- Panic hook : tout panic (thread UI ou worker) est écrit dans le log (emplacement et
  message, texte masqué par `redact`).
- Worker : nouvelles commandes `LoadLog { skip }`, `LoadCommit(id)`, `LoadCommitFileDiff { id, path }`,
  `LoadBranches`, `CreateBranch { name, switch }`, `SwitchBranch { name, stash: bool }`,
  `RenameBranch { old, new }`, `DeleteBranch { name, force }`, `CheckoutRemote(name)`,
  `Fetch`, `Pull(PullMode)`, `Push(PushMode)`, `AbortOperation`, `ContinueRebase`,
  `LoadSigning` ; annulation réseau via `WorkerHandle::cancel_network()`.
  - Après toute opération qui change HEAD ou les refs : `RepoOpened(summary)`,
    `StatusLoaded`, `BranchesLoaded`, et rechargement de la première page d'historique.
  - Fetch automatique une fois à l'ouverture d'un dépôt (si connecté ou si le remote n'est
    pas github.com HTTPS) ; aucun fetch périodique.
- État : `HistoryView { entries, graph, selected: Option<String>, detail, detail_file, detail_diff, loading, end_reached }`,
  `BranchesView { branches, loading }`, `SyncView { running: Option<SyncOp>, progress }`,
  `ChangesView.operation: Option<Operation>`, `ChangesView.signing: Option<SigningConfig>`,
  dialogues en attente (`PendingDialog::{Diverged, PushRejected, WouldOverwrite, DeleteNotMerged, NewBranch, RenameBranch}`).
- UI :
  - Onglets « Changes » / « History » dans le panneau central.
  - Barre d'outils : Fetch, Pull ↓n, Push ↑n (Publish branch sans upstream), liste
    déroulante des branches (locales puis distantes), New branch.
  - Menu **Repository** : Fetch, Pull, Push, New branch…, Rename branch…, Delete branch…,
    Abort merge/rebase.

## 4. Écrans

Résumé des écrans validés en conversation :

- **History** : graphe + id court + résumé + étiquettes de refs + auteur + date relative ;
  détail en bas (splitter) avec message, signature, fichiers, diff lecture seule.
- **Changes** : indicateur « 🔒 Signed with GPG key … » ou « Commits will NOT be signed »
  (rouge) près de [Commit] ; bandeau jaune « Merge in progress: resolve conflicts, stage,
  then commit » avec [Abort merge] (ou [Abort rebase] / [Continue rebase]).
- **Dialogues** : pull divergent (Merge / Rebase / Cancel) ; push refusé (Pull / Cancel,
  plus « Force push (with lease)… » avec seconde confirmation si HEAD a été amendé) ;
  bascule impossible (Stash, switch and re-apply / Cancel) ; suppression (confirmation,
  « Not merged: delete anyway? ») ; nouvelle branche et renommage (champ validé, case
  « Switch to it ») ; progression réseau (phase + barre + Cancel).

## 5. Flux de données

1. Ouvrir un dépôt : `RepoOpened`, `StatusLoaded`, `BranchesLoaded`, première page
   d'historique, puis fetch automatique en arrière-plan (barre d'état « Fetching… »).
2. Défiler l'historique près de la fin charge la page suivante (`LoadLog { skip }`) ;
   le graphe est prolongé par la variante incrémentale.
3. Pull : `Pull(FastForwardOnly)` → `Diverged` → dialogue → `Pull(Merge|Rebase)` →
   `PullOutcome` ; `Conflicts` → bandeau et fichiers `[!]` dans Changes.
4. Push : `Push(Normal)` → `PushRejected` → dialogue ; sans upstream, le bouton envoie
   `Push(SetUpstream)`.
5. Bascule : `SwitchBranch { stash: false }` → `WouldOverwrite` → dialogue →
   `SwitchBranch { stash: true }` (stash push, switch, stash pop ; `StashConflict` signalé).

## 6. Erreurs et cas limites

| Situation | Comportement |
|---|---|
| `git` absent | Historique et branches en lecture seule ; les autres actions sont grisées avec « Install Git to use this feature ». |
| Invite d'identifiants (HTTPS non github.com, SSH sans agent) | Jamais bloquant : échec rapide avec la sortie de Git et une aide (« Add your SSH key to ssh-agent » / « Configure a credential helper »). |
| Token RetroGit refusé (401) | Même traitement que le sous-projet 1 : token effacé, reconnexion demandée. |
| SSO non autorisé | Message SSO avec lien (réutilise l'existant). |
| Pull divergent | Dialogue Merge / Rebase / Cancel. |
| Conflits après pull ou stash pop | Message ; fichiers `[!]` ; bandeau Abort / Continue ; stash conservé si besoin. |
| Push non fast-forward | Dialogue Pull / Cancel ; force-with-lease seulement après confirmation explicite. |
| Branche sans upstream | Bouton « Publish branch » (`push -u origin <branche>`). |
| HEAD détachée | Pull et Push grisés ; liste des branches indique « (detached at abc1234) ». |
| Nom de branche invalide ou déjà pris | Erreur sous le champ, bouton OK grisé. |
| Suppression de la branche courante | Refusée (« Switch to another branch first »). |
| Suppression d'une branche non fusionnée | Seconde confirmation, puis `-D`. |
| Signature activée et `git` absent | Commit refusé (`SigningRequiresGit`) : jamais de commit non signé. |
| Signature qui échoue (passphrase annulée, agent absent) | Sortie de Git affichée ; message conservé (existant). |
| Annulation réseau | Processus tué ; message « Cancelled » ; état « en cours » signalé si un merge/rebase a commencé. |
| Très gros historique | Pages de 500, virtualisation, graphe incrémental. |

## 7. Tests

- **`graph::layout`** (pur) : historique linéaire ; branche puis merge ; deux branches
  parallèles ; merge octopus ; racines multiples ; réutilisation des couloirs ; continuité
  entre deux pages (incrémental = calcul en une fois).
- **`parse_progress`**, **`askpass_answer`**, **`validate_branch_name`** (purs).
- **`gitcore`** (intégration, dépôts temporaires, remote nu local en `file://`) : log et
  refs ; détail et diff d'un commit ; signature_status sur un commit non signé ; branches
  (create / switch / rename / delete / non fusionnée / checkout remote / ahead-behind) ;
  WouldOverwrite puis stash-switch-pop ; StashConflict ; fetch ; pull ff / divergent / merge
  / rebase / conflits ; push / rejeté / set-upstream / force-with-lease ; abort merge et
  rebase ; annulation d'un fetch ; `signing_config` ; commit refusé sans git quand la
  signature est requise. Tests ignorés si `git` absent.
- **Askpass** : test du binaire lancé avec `--askpass "Username for 'https://github.com':"`
  puis `--askpass "Password for ...":` via `std::process::Command` (réponses
  `x-access-token` puis le token lu dans `RETROGIT_ASKPASS_TOKEN`), et test que la commande
  réseau construite pour une URL github.com HTTPS contient `-c credential.helper=` et
  `GIT_ASKPASS`, mais pas pour une URL SSH.
- **`app`** : reducer (historique paginé, sélection, dialogues, SyncView, bandeau
  d'opération) ; worker (ouvrir → branches + log ; pull divergent → événement ; push rejeté
  → événement) avec un remote `file://`.
- **`win95`** : kittest pour `splitter` (déplacement) et `combo_box` (sélection).

## 8. Build

Aucune nouvelle dépendance prévue. Binaire release sous 15 Mo.
