# RetroGit — Sous-projet 1 : Socle + connexion GitHub

- **Date** : 2026-09-30
- **Statut** : design validé en conversation, en attente de revue de la spec écrite

## 1. Contexte et objectif

RetroGit est un client Git de bureau écrit en Rust, avec une interface au look
**Windows 95/98** (gris 3D biseauté, barre de titre bleue en dégradé, police
bitmap type MS Sans Serif). C'est un **outil quotidien** pour son auteur, pas
(encore) un produit publié.

Cibles obligatoires dès le premier jour : **macOS arm64** et **Windows x86_64**.
Priorités non fonctionnelles : **performances** et **légèreté**.

Le projet complet est découpé en 4 sous-projets, chacun avec sa propre
spec → plan → implémentation :

1. **Socle + connexion GitHub** (ce document)
2. Travail local : status, diff, stage/unstage, commit
3. Synchro + historique : fetch, pull, push, log, branches
4. Pull Requests GitHub : lister, voir, créer, relire

### Critères de succès du sous-projet 1

1. L'application se lance sur macOS arm64 et Windows x86_64 avec une fenêtre
   au look Win95 (barre de titre custom, menus, barre d'outils, panneaux,
   barre d'état).
2. L'utilisateur se connecte à github.com via OAuth Device Flow (SSO SAML de
   l'organisation compris), ou en collant un PAT en secours.
3. Le token est stocké dans le coffre de l'OS et survit au redémarrage de l'app.
4. L'utilisateur voit la liste de ses dépôts (perso + organisations), peut
   filtrer, et cloner un dépôt avec progression et annulation.
5. L'utilisateur peut ouvrir un dépôt local existant ; les dépôts ouverts ou
   clonés apparaissent dans une liste de récents persistée.
6. L'interface ne bloque jamais pendant une opération réseau ou Git ; CPU ≈ 0 %
   au repos.

## 2. Décisions techniques

| Sujet | Choix | Raison principale |
|---|---|---|
| Interface | `egui` / `eframe` | Rust pur, rendu identique sur les deux OS, dessin custom total, rapide |
| Moteur Git | `git2` (libgit2 embarqué) derrière une API interne | Seul choix couvrant tout le périmètre dans un binaire autonome |
| Repli futur | `git` CLI (hooks `pre-commit`), `gix` pour les chemins chauds | Rendu possible par l'API interne ; **hors périmètre S1** |
| HTTP | `ureq` + `rustls`, synchrone | Pas de runtime async, binaire léger |
| Auth | OAuth Device Flow (OAuth App perso) + PAT en secours | Vraie UX d'app, fonctionne même si l'OAuth App est bloquée |
| Stockage du token | `keyring` (Trousseau macOS / Gestionnaire d'identifiants Windows) | Jamais en clair sur disque |
| Sélecteur de dossier | `rfd` | Boîte native de l'OS |
| Erreurs | `thiserror` par crate | Erreurs typées, messages clairs |
| Langue de l'interface | Anglais, libellés centralisés dans un module | i18n possible plus tard |

## 3. Architecture

Workspace Cargo dans `~/perso/retrogit` :

```
retrogit/
├── Cargo.toml            # workspace + profils
├── crates/
│   ├── win95/            # kit de widgets Win95 pour egui (aucune logique métier)
│   ├── gitcore/          # API Git interne, implémentée avec git2
│   ├── github/           # client GitHub : Device Flow, API REST, stockage du token
│   └── app/              # binaire `retrogit` : écrans, état, orchestration
└── docs/superpowers/specs/
```

Règles de dépendance : `app` → `win95`, `gitcore`, `github`. Les trois crates
de bibliothèque ne dépendent **pas** les unes des autres.

### 3.1 `win95`

- `theme` : palette Win95 (gris `#C0C0C0`, blanc, gris foncé `#808080`, noir,
  bleu titre `#000080` → `#1084D0`), police bitmap embarquée, application aux
  `egui::Visuals` et `Style`.
- Widgets : `button` (biseau en relief, enfoncé au clic, état désactivé),
  `sunken_panel`, `raised_panel`, `title_bar` (dégradé, boutons _ □ X,
  zone de déplacement de la fenêtre), `menu_bar`, `toolbar`, `status_bar`
  (cellules enfoncées), `list_view` (colonnes, sélection, virtualisation des
  lignes), `tree_view`, `text_field`, `tabs`, `progress_bar` (blocs bleus),
  `message_box` (icônes erreur / avertissement / info), `window_frame`.
- Police : **W95FA** (licence OFL). La licence doit être vérifiée et le fichier
  `OFL.txt` embarqué avant d'intégrer la police. Si W95FA ne convient pas,
  choisir une autre police libre (OFL ou équivalent) qui imite MS Sans Serif.
  Aucune police Microsoft propriétaire n'est embarquée.

### 3.2 `gitcore`

API publique (types propres à nous, jamais de type `git2` exposé) :

- `Repo::open(path) -> Result<Repo, GitError>`
- `Repo::summary() -> Result<RepoSummary, GitError>` : branche courante (ou HEAD
  détachée), URL du remote `origin`, dernier commit (id court, résumé, auteur, date).
- `clone(req: CloneRequest, progress: impl FnMut(CloneProgress), cancel: &AtomicBool) -> Result<Repo, GitError>`
  - `CloneRequest { url, dest, credentials: Option<Credentials> }`
  - `CloneProgress { received_objects, total_objects, received_bytes, indexed_deltas, total_deltas }`
- `GitError` : `NotARepository`, `DestinationNotEmpty`, `Cancelled`,
  `Auth`, `Network`, `Other(String)`.

Configuration `git2` : `default-features = false`, features `https` et
`vendored-libgit2`, **sans SSH**. Sous macOS et Windows, HTTPS passe par la pile
TLS native de l'OS (SecureTransport / WinHTTP). La première tâche du plan
vérifie cette configuration de features sur les deux OS.

### 3.3 `github`

- `DeviceFlow` : **machine à états pure**, sans I/O.
  - Entrée : réponses de `POST /login/device/code` et de
    `POST /login/oauth/access_token`.
  - Sortie : prochaine action (`Poll { after }`, `Done(token)`,
    `Failed(reason)`).
  - Gestion de `authorization_pending`, `slow_down` (+5 s à l'intervalle),
    `expired_token`, `access_denied`.
- `Client` (HTTP via `ureq`) :
  - `request_device_code(client_id, scopes)`, `poll_token(...)`
  - `current_user() -> User`
  - `list_repos() -> Vec<RepoInfo>` : `GET /user/repos?affiliation=owner,collaborator,organization_member&sort=updated&per_page=100`,
    pagination via l'en-tête `Link`.
  - L'URL de base est configurable (tests contre un faux serveur).
- `TokenStore` : trait avec une implémentation `keyring` (service
  `RetroGit`, compte `github.com`) et une implémentation mémoire pour les tests.
- `GithubError` : `Unauthorized` (401), `SsoRequired { url }` (403 avec en-tête
  `X-GitHub-SSO`), `RateLimited`, `Network`, `Http(status)`, `Decode`.
- Scopes demandés : `repo`, `read:org`.
- Le `client_id` de l'OAuth App est public et embarqué dans le binaire via une
  constante. Aucun `client_secret`.

### 3.4 `app`

- Fenêtre `eframe` **sans décoration native**. La barre de titre Win95 gère le
  déplacement, la réduction, l'agrandissement et la fermeture via les
  `ViewportCommand` d'egui. Mode de rendu **réactif** (pas de repaint continu).
- **Modèle d'exécution** : un thread worker unique.
  - UI → worker : `Command` (`ValidateToken`, `StartDeviceFlow`,
    `CancelDeviceFlow`, `SavePat(token)`, `SignOut`, `ListRepos`,
    `Clone { url, dest }`, `CancelClone`, `OpenRepo(path)`).
  - Worker → UI : `Event` (`SignedIn(User)`, `SignedOut`,
    `DeviceCode { user_code, verification_uri }`, `ReposLoaded(..)`,
    `CloneProgress(..)`, `CloneDone(RepoSummary)`, `RepoOpened(RepoSummary)`,
    `Error(AppError)`).
  - Canaux `std::sync::mpsc`. Après chaque `Event`, le worker appelle
    `ctx.request_repaint()`.
  - L'annulation passe par des `Arc<AtomicBool>` partagés.
- **État** : `AppState` mis à jour par une fonction pure
  `apply(state, event) -> state`, testée unitairement.
- **Config** persistée en JSON dans le dossier de config de l'OS (`dirs`) :
  dépôts récents (chemin + nom), dernier dossier de destination de clone,
  taille et position de la fenêtre. **Jamais le token.**
- **Libellés** : tous dans un module `strings`, en anglais.

## 4. Écrans

### 4.1 Fenêtre principale

```
┌─■ RetroGit - acme-cor-lab ─────────────────[_][□][X]┐
│ File  Repository  View  Help                        │
│ [Clone] [Open] | [Fetch] [Pull] [Push]              │  ← Fetch/Pull/Push désactivés au S1
├──────────────┬──────────────────────────────────────┤
│ Repositories │  acme-cor-lab                        │
│ ▸ acme-cor…  │  Branch : main                       │
│ ▸ retrogit   │  Remote : github.com/ExampleOr…        │
│              │  Last commit : a1b2c3 "fix …"        │
│              │  (zone réservée au S2)               │
├──────────────┴──────────────────────────────────────┤
│ Signed in: @user              │ Ready               │
└─────────────────────────────────────────────────────┘
```

- Menu **File** : Sign in… / Sign out, Clone…, Open…, Exit.
- Menu **Repository** : entrées du S2/S3 affichées mais désactivées.
- Menu **Help** : About RetroGit.
- Panneau gauche : dépôts récents. Un clic ouvre le dépôt et affiche son
  résumé. Un dépôt introuvable sur disque est grisé, avec une entrée de menu
  contextuel « Remove from list ».

### 4.2 Boîte « Sign in to GitHub »

- Ouverte automatiquement au lancement s'il n'y a pas de token valide, ou via
  File > Sign in….
- Onglet **Standard** : code utilisateur en grand, [Copy code],
  [Open browser], barre de progression animée pendant l'attente, [Cancel].
- Onglet **Advanced** : champ masqué pour le PAT, [OK]. Le token est validé
  par `GET /user` avant d'être stocké.

### 4.3 Boîte « Clone a repository »

- Champ de filtre (sous-chaîne, insensible à la casse, sur `owner/name`).
- `list_view` : colonnes Name / Owner / Private / Updated, virtualisée.
- Bouton [Refresh].
- Champ « Destination folder » + [Browse…] (`rfd`). Par défaut, le dernier
  dossier utilisé, auquel on ajoute le nom du dépôt.
- [Clone] ouvre une boîte de progression façon « Copying files » : barre de
  progression, nombre d'objets, Mo reçus, [Cancel].

### 4.4 Ouvrir un dépôt local

File > Open… → `rfd` choix de dossier → `Repo::open`. Si ça réussit, le dépôt
est ajouté aux récents et affiché. Sinon, une message box `NotARepository`
s'affiche.

## 5. Flux de données

### 5.1 Démarrage

1. Lecture de la config.
2. `Command::ValidateToken` : lecture du token dans `TokenStore`, puis
   `GET /user`.
   - 200 → `SignedIn(user)`.
   - 401 → suppression du token, `SignedOut`, ouverture de la boîte de connexion.
   - Pas de token → ouverture de la boîte de connexion.
   - Erreur réseau → message box d'avertissement. Les fonctions locales
     (ouvrir un dépôt) restent utilisables.

### 5.2 Device Flow

`StartDeviceFlow` → `request_device_code` → `DeviceCode` affiché → boucle de
polling pilotée par la machine à états (en respectant `interval`, annulable)
→ `Done(token)` → stockage dans `TokenStore` → `GET /user` → `SignedIn`.

### 5.3 Clone

`Clone { url, dest }` :
1. Vérifier que `dest` n'existe pas ou est vide, sinon `DestinationNotEmpty`.
2. Noter si c'est nous qui créons `dest`.
3. `gitcore::clone` avec l'URL HTTPS **sans token**. Les identifiants passent
   par le callback git2 (utilisateur `x-access-token`, mot de passe = token).
   Le token n'apparaît jamais dans `.git/config`.
4. La progression est transmise par `CloneProgress`, au maximum environ
   20 fois par seconde.
5. En cas d'échec ou d'annulation : suppression de `dest` **seulement si c'est
   l'app qui l'a créé**. Sinon, on vide uniquement ce que le clone a écrit dans
   ce dossier, qui était vide au départ.
6. En cas de succès : ajout aux récents, `CloneDone(summary)`.

## 6. Gestion des erreurs

- Toute erreur remontée à l'UI devient un `AppError { kind, message, detail }`,
  affiché dans une `message_box` Win95 avec l'icône adaptée et un bouton [OK].
- Cas prévus avec un message dédié :
  - pas de réseau ;
  - token invalide ou révoqué (→ reconnexion) ;
  - **SSO requis** (lien `X-GitHub-SSO` cliquable) ;
  - limite de requêtes atteinte ;
  - dossier de destination non vide ;
  - dossier qui n'est pas un dépôt ;
  - clone annulé (info, pas une erreur).
- **Log** : fichier texte en rotation simple (un fichier, tronqué au-delà de
  5 Mo) dans le dossier de données de l'OS. Tout token est masqué avant
  écriture (jamais écrit en clair).
- Aucun `unwrap()` / `expect()` sur des chemins d'exécution normaux. Le thread
  worker ne doit jamais faire paniquer l'app : ses panics sont capturés et
  convertis en `Error`.

## 7. Tests

Aucun test ne dépend du réseau.

- **`gitcore`** : tests d'intégration avec `tempfile` : `open` sur un dépôt et
  sur un dossier qui n'en est pas un ; `summary` (branche, HEAD détachée, dépôt
  sans commit) ; `clone` depuis un dépôt nu local (`file://`) : succès,
  progression reçue, annulation, destination non vide.
- **`github`** : tests unitaires de la machine `DeviceFlow` (toutes les
  transitions). Tests du `Client` contre `mockito` : `current_user`,
  pagination de `list_repos`, 401, 403 SSO, rate limit.
- **`app`** : tests unitaires de `apply(state, event)`, et de la
  lecture/écriture de la config (y compris un fichier corrompu → config par
  défaut).
- **`win95`** : quelques tests d'interaction `egui_kittest` (clic sur un
  bouton, bouton désactivé, sélection dans `list_view`). Pas de captures de
  référence en v1.

## 8. Build et CI

- Profil release : `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`,
  `strip = true`, `panic = "unwind"` (nécessaire pour capturer les panics du
  worker).
- Objectif indicatif : binaire release de 10 à 15 Mo environ.
- Build natif sur chaque OS : `aarch64-apple-darwin` et
  `x86_64-pc-windows-msvc`. Sous Windows, le binaire utilise le sous-système
  GUI (pas de console qui s'ouvre).
- Workflow GitHub Actions (`macos-14`, `windows-latest`) : `fmt --check`,
  `clippy -D warnings`, `test`, build release, publication des artefacts.
  **Le workflow est écrit mais le dépôt reste local pour l'instant** : il ne
  sera exécuté qu'une fois le dépôt poussé sur GitHub. En attendant, la
  vérification Windows se fait manuellement.

## 9. Prérequis manuels

- Créer une OAuth App sur le compte GitHub de l'utilisateur (callback URL
  quelconque, **Enable Device Flow** coché), puis renseigner son `client_id`
  dans le code.
- À la première connexion, autoriser l'app pour l'organisation SSO
  (ExampleOrg) sur l'écran de consentement GitHub.

## 10. Hors périmètre du sous-projet 1

- Status, diff, commit (S2) ; fetch, pull, push, log, branches (S3) ; PR (S4).
- SSH, plusieurs comptes, GitHub Enterprise Server.
- Repli vers le `git` CLI et vers `gix` (prévu plus tard, rendu possible par
  l'API `gitcore`).
- Signature de code et notarisation (avertissements Gatekeeper et SmartScreen
  acceptés), mise à jour automatique, traductions.
- Récupération du token du CLI `gh`.
