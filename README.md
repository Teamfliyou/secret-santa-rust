# 🎄 Secret Santa — application web pour une classe

Application complète en **Rust** (Axum + Askama + SQLite/SQLx), responsive et mobile-first, avec une animation d'enveloppe qui tombe du ciel pour révéler son destinataire.

- Comptes élèves créés **uniquement par l'administrateur** (identifiant seul ; chaque élève choisit son mot de passe à la première connexion, hash Argon2id). Une seule page de connexion pour tous.
- Questionnaire de préférences, modifiable à tout moment.
- Tirage au sort (dérangement aléatoire) **contrôlé par l'administrateur**, atomique et définitif.
- Interrupteur « Autoriser les élèves à découvrir leur personne », vérifié côté serveur.
- Attribution mémorisée en base : on retrouve toujours le même destinataire (`revealed_at` enregistré à la première ouverture).
- Sécurité : sessions serveur, cookies `HttpOnly` / `SameSite` / `Secure`, CSRF, limitation des tentatives, CSP stricte, journal d'audit.

## 1. Installer le projet

Prérequis : [Rust](https://rustup.rs) stable (≥ 1.80) et un compilateur C (SQLite est compilé avec le projet).

```bash
git clone <ton-depot> secret-santa && cd secret-santa
cp .env.example .env
```

## 2. Variables d'environnement

Voir [`.env.example`](.env.example) :

| Variable | Rôle | Défaut |
|---|---|---|
| `ADMIN_USERNAME` | identifiant admin | `admin` |
| `ADMIN_PASSWORD` | mot de passe admin (voir §3) | généré aléatoirement |
| `DATABASE_PATH` | fichier SQLite | `data/santa.db` |
| `BIND_ADDR` | adresse d'écoute | `0.0.0.0:3000` |
| `COOKIE_SECURE` | `true` en HTTPS (production) | `false` |
| `TRUST_PROXY` | `true` derrière un reverse proxy de confiance | `false` |
| `SESSION_DAYS` | durée d'une session | `30` |

## 3. Initialiser l'administrateur

Au premier démarrage, le compte admin est créé :

- avec `ADMIN_PASSWORD` si tu l'as défini dans `.env` ;
- sinon avec un mot de passe aléatoire **affiché une seule fois dans les logs** (`docker compose logs secret-santa`).

`ADMIN_PASSWORD` fait foi : si tu le modifies puis redémarres, le mot de passe est mis à jour (et les sessions admin sont coupées). L'espace admin est sur **`/admin`** (la page de connexion est la même pour tout le monde : `/login`).

## 4. Lancer le site

Développement :

```bash
cargo run
# → http://localhost:3000   (admin : http://localhost:3000/admin)
```

Production (Docker) :

```bash
docker compose up -d --build
```

La base SQLite est dans le volume Docker `santa-data` (`/data/santa.db`) et survit aux redémarrages et mises à jour. Le conteneur n'écoute que sur `127.0.0.1:3000` : place un reverse proxy HTTPS devant (§6). Endpoint de santé : `GET /health`.

Pensez à mettre `COOKIE_SECURE=true` (et `TRUST_PROXY=true`) dans `.env` en production.

## 5. Déroulé conseillé et import des élèves

1. **Paramètres** (`/admin/settings`) : nom de la classe, de l'événement, budget, date de remise, message d'accueil.
2. **Élèves** (`/admin/students`) :
   - ajout un par un, ou **import CSV** : une ligne par élève `prénom,nom` (séparateur `,` ou `;`, en-tête facultatif, identifiant facultatif en 3ᵉ colonne). Exemple :
     ```csv
     prenom,nom
     Emma,Martin
     Lucas,Dupont
     ```
   - l'identifiant (`emma.martin`) est généré ; tu ne donnes **que l'identifiant** (boutons Copier / Imprimer sur la page de résultat).
   - **Première connexion** : l'élève entre son identifiant et laisse le mot de passe vide, puis **choisit lui-même son mot de passe** (8 caractères minimum). Tu ne connais jamais les mots de passe. Attention : tant qu'un élève n'a pas choisi son mot de passe, quiconque connaît son identifiant peut le faire à sa place ; fais activer les comptes rapidement.
   - « Réinitialiser le mot de passe » remet le compte dans cet état (sessions coupées) : l'élève en choisit un nouveau.
3. Les élèves se connectent sur `/`, remplissent leur questionnaire.
4. **Tirage** (`/admin/draw`) : le bouton « 🎲 Effectuer le tirage au sort » n'est actif que lorsque tous les questionnaires sont remplis (la liste des retardataires est affichée). Une confirmation est demandée ; le tirage est définitif.
5. Quand tu es prêt, active **« Autoriser les élèves à découvrir leur personne »**. Tu peux le désactiver à tout moment sans toucher aux attributions.
6. En cas de problème, **« Consulter les attributions »** (mot de passe admin redemandé, action enregistrée dans le journal `/admin/audit`).

## 6. HTTPS via reverse proxy

Exemple **Caddy** (certificat Let's Encrypt automatique) :

```caddyfile
santa.mondomaine.fr {
    reverse_proxy 127.0.0.1:3000
}
```

Exemple **Nginx** (avec Certbot) :

```nginx
server {
    server_name santa.mondomaine.fr;
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Puis dans `.env` : `COOKIE_SECURE=true` et `TRUST_PROXY=true`, et `docker compose up -d`.

## 7. Sauvegarder la base

```bash
# sauvegarde à chaud, cohérente (ne nécessite pas d'arrêter le site)
docker run --rm -v secret-santa_santa-data:/data -v "$PWD":/backup alpine \
  sh -c 'apk add --no-cache sqlite >/dev/null && sqlite3 /data/santa.db ".backup /backup/santa-$(date +%F).db"'
```

Restauration : arrêter le service, remplacer `/data/santa.db` par la sauvegarde (supprimer aussi `santa.db-wal` et `santa.db-shm`), relancer. En développement, il suffit de copier `data/santa.db` (service arrêté).

## 8. Mettre à jour sans perdre les données

```bash
git pull
docker compose up -d --build
```

Les migrations SQL (`migrations/`) sont appliquées automatiquement au démarrage et les données restent dans le volume. Fais une sauvegarde (§7) avant une mise à jour majeure.

## Architecture

```
src/
  main.rs          démarrage (config, base, serveur)
  lib.rs           état, migrations, admin initial, routeur, en-têtes de sécurité
  auth.rs          sessions, extracteurs (élève / admin), CSRF, limiteur de tentatives
  draw.rs          dérangement aléatoire + tirage transactionnel
  web/             handlers : public (login), student, admin
  templates.rs     structures Askama      →  templates/*.html
migrations/        schéma SQLite (contraintes CHECK/UNIQUE/FK + triggers)
static/            style.css, app.js (animations), early.js
tests/app.rs       tests d'intégration de bout en bout
```

Points de sécurité notables :

- Les droits sont toujours déduits de la **session serveur** ; aucun identifiant d'URL ne détermine quel destinataire est affiché.
- Triggers SQL : les attributions ne peuvent être créées qu'après le tirage, ne sont ni modifiables ni supprimables ; `giver_id` et `receiver_id` sont uniques et distincts.
- Le tirage commence par une écriture conditionnelle dans une transaction : deux demandes simultanées ne peuvent pas produire deux tirages.
- CSP `default-src 'self'` (aucun script/style inline), `X-Frame-Options: DENY`, `nosniff`.

## Tests et qualité

```bash
cargo fmt
cargo check
cargo test
cargo clippy --all-targets
```

> Sous Windows avec la toolchain `*-gnu`, un chemin de projet très long peut faire échouer la compilation (`dlltool`) : définis `CARGO_TARGET_DIR` vers un chemin court, par ex. `C:\t\santa`.
