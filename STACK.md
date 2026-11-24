# Lanceur — stack technique (document de référence)

Version 0.1.0, v1 livrée vite (consigne du 24/11) : pas encore d'audit ponytail ni d'EGM.

## 1. But et direction
- **Rôle** : une barre centrale ouverte par **Alt+Espace** (remplace le menu système de fenêtre). On tape 2 lettres, Entrée.
- **Cibles** : ouverture < 50 ms, recherche < 5 ms, CPU ≈ 0 au repos.
- **Vie** : tourne en fond, icône dans la zone de notification (clic : ouvrir ; clic droit : démarrer avec Windows, quitter), démarre avec Windows (entrée `HKCU\…\Run\Lanceur` = `"<exe>" --fond`, écrite par l'exe lui-même au lancement). Un lancement simple reste en fond ; un 2e lancement ouvre la barre.
- **Hors périmètre v1** : apps du Store (UWP) non listées (seulement les `.lnk` du menu Démarrer) ; palette Ctrl+K et F1 (la barre en tient lieu).

## 2. Stack
| Domaine | Choix |
|---|---|
| Langage | Rust 2024, rustc 1.99.1 MSVC |
| UI | egui 0.37 + winit 0.30 + rendu CPU (`hdrshot-egui-backend` + `softbuffer`), boucle reprise de Bilan |
| Socle | v0.1.6, features `ui settings crash instance update testmode` (recherche floue `socle::fuzzy`) |
| Win32 | `windows-sys` 0.61 (raccourci, icône, registre, TCP, presse-papiers, ShellExecute) ; `windows` 0.62 (COM : Core Audio, IPolicyConfig, UI Automation) |
| Réseau | `curl.exe` de Windows, une fois par jour, pour les taux BCE |

## 3. Architecture
| Fichier | Rôle |
|---|---|
| `src/main.rs` | boucle winit, fenêtre sans bord toujours au-dessus, cachée/montrée (jamais recréée), `--action`, instance unique, mode test (`LANCEUR_REQUETE`, `LANCEUR_CAPTURE`, `LANCEUR_ATTENTE`, `LANCEUR_MESURE`) |
| `src/fond.rs` | thread à boucle de messages : `RegisterHotKey(Alt+Espace)`, icône `Shell_NotifyIcon` + menu, lecture de la sélection (35 ms max avant d'ouvrir), clé Run |
| `src/app.rs` | réglages, données chargées en fond, travaux lents (`Req`) sur threads, dessin de la barre, clavier, exécution |
| `src/moteur.rs` | requête → résultats (pur, testé) : préfixes, commandes, sélection, apprentissage (`Appris`), clavier inversé |
| `src/sources.rs` | menu Démarrer, apps maison, jeux (`library.json` d'Ultimate, sinon Steam/Epic), projets, index de fichiers (Explorateur `EXPLIDX2` filtré, ou parcours à soi) |
| `src/integr.rs` | détection des apps maison et de leurs CLI (marqueurs de la ressource de version), Vigie / Mémoire / Bilan / sessions Claude / sorties audio, exécution des actions, taux BCE |
| `src/calc.rs` | expressions, %, unités, devises, couleurs (std seulement) |
| `src/sysw.rs` | ports en écoute, tuer (chemin vérifié juste avant), verrouiller, veille, corbeille, ShellExecute, sortie de CLI avec délai, presse-papiers |
| `src/audio.rs` | volume, muet, sorties, sortie par défaut (IPolicyConfig, interface non documentée) |
| `src/uia.rs` | texte sélectionné par UI Automation (TextPattern), délais de 100 ms |

## 4. Direction artistique
Thème du socle (crème / bleu nuit selon Windows), carte arrondi 10, champ 20 px, lignes de 46 px (titre 15, sous-titre 12 `muted`), lettres trouvées en `accent`, ligne active `sel_bg`, icônes vectorielles du socle, aperçu de couleur pour `#hex`. Icône : barre crème avec chevron bleu sur fond bleu nuit (`assets/make_icon.py`).

## 5. Conventions
- Réglages `%APPDATA%\Lanceur\settings.json` : `demarrage`, `appris` (usage par clé, dernier choix par requête), `snippets` (`;nom` → texte copié), `derniere` (pour `!`). Cache `%LOCALAPPDATA%\Lanceur\taux-bce.xml`.
- Clavier : ↑↓, Entrée, Maj+Entrée (dossier), Tab (compléter), Échap (vider puis fermer). Une action risquée (veille, tuer, mode jeu, couper un serveur) demande Entrée deux fois.
- Processus externes sans console (`CREATE_NO_WINDOW`) et avec délai ; tuer un processus : chemin de l'exe revérifié sur la poignée.
- Mode test (§0) : ni raccourci global, ni icône, ni clé Run ; fenêtre créée inactive, opacité 0 avant d'apparaître.

## 6. Fonctionnalités
Base (apps, jeux, fichiers, Maj+Entrée), classement appris, calcul/conversions/devises/couleurs, commandes Windows, préfixes `g yt gh`, branchements `y v k ram mode jeu p bilan`, dev `d <projet> [t|gh]`, `;snippets`, `:port`, `c` (sessions Claude Code reprises par `claude --resume` dans Windows Terminal), actions sur la sélection, clavier AZERTY↔QWERTY, `!`, `--action`. Mise à jour par `socle::update` (une ligne « Mettre à jour » apparaît dans la barre vide).

Intégrations (règle 4, masquées si absentes) :
| Préfixe | App | Condition |
|---|---|---|
| `y` | `Tube.exe "<texte>"` | exe présent |
| `v` | `Memoire.exe --chercher "<t>" --json`, `--copier <id>` | marqueur `MemoireCli` contient `chercher` (**CLI demandée, pas encore faite**) |
| `k`, `ram` | `Vigie.exe --etat-dev`, `--couper-dev --seulement <cle>` | `VigieCli` contient `etat-dev couper-dev` ; sinon `k :port` en direct |
| `mode jeu` | `Vigie.exe --avant-jeu` / `--apres-jeu` | exe présent |
| `bilan` | `Bilan.exe --resume` | marqueur `BilanCli` contient `resume` (**CLI demandée, pas encore faite**) |
| `c` | `claude.exe` (PATH ou `%APPDATA%\Claude\claude-code\…`) | exe trouvé |

## 7. Mesures (24/11, i9-9900K, mode test invisible)
| Mesure | Valeur |
|---|---|
| Ouverture (montrer → 1re image présentée) | 14 ms |
| Recherche apps/jeux/commandes | 0,1 ms |
| Recherche fichiers (517 121 entrées, index de l'Explorateur filtré) | 4 à 6,5 ms (en fond, ajoutée après) |
| `--action` sans instance ouverte | 150 ms |
| RAM privée (index de fichiers chargé) | 65,6 Mo |
| Exe release | 5,6 Mo |
| CPU au repos | non mesuré (boucle `ControlFlow::Wait`, aucun sondage) |

## 8. Limites connues
- Non vérifiés à la main (§0 : aucune entrée simulée) : Alt+Espace réel, prise de focus, lecture de la sélection dans de vraies apps, volume/muet/sortie audio (IPolicyConfig jamais exécuté), verrouiller/veille/corbeille, menu de l'icône.
- La sélection est lue avant d'ouvrir la barre (35 ms max) ; une app lente répond trop tard et la sélection arrive après coup (ou pas).
- Noms de fichiers en minuscules quand ils viennent de l'index de l'Explorateur (il les stocke repliés).
- RAM : l'index de fichiers en coûte l'essentiel.
- `cargo test` : 32 tests (moteur, calcul, sources, marqueurs, Win32 en lecture seule).

## 9. Pistes (à valider)
- Apps du Store (shell:AppsFolder), Ctrl+chiffre pour choisir la n-ième ligne, aperçu de fichier par le Lecteur, historique de calculs.
