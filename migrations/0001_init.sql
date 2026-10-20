-- Secret Santa : schéma initial

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL COLLATE NOCASE UNIQUE
                  CHECK (length(username) BETWEEN 2 AND 64),
    first_name    TEXT NOT NULL CHECK (length(trim(first_name)) BETWEEN 1 AND 60),
    last_name     TEXT NOT NULL CHECK (length(trim(last_name)) BETWEEN 1 AND 60),
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'student' CHECK (role IN ('student', 'admin')),
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE preferences (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id          INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    interests        TEXT NOT NULL DEFAULT '' CHECK (length(interests) <= 1000),
    hobbies          TEXT NOT NULL DEFAULT '' CHECK (length(hobbies) <= 1000),
    gift_types       TEXT NOT NULL DEFAULT '' CHECK (length(gift_types) <= 1000),
    favorite_snacks  TEXT NOT NULL DEFAULT '' CHECK (length(favorite_snacks) <= 1000),
    favorite_colors  TEXT NOT NULL DEFAULT '' CHECK (length(favorite_colors) <= 1000),
    wishlist         TEXT NOT NULL DEFAULT '' CHECK (length(wishlist) <= 1000),
    dislikes         TEXT NOT NULL DEFAULT '' CHECK (length(dislikes) <= 1000),
    food_restrictions TEXT NOT NULL DEFAULT '' CHECK (length(food_restrictions) <= 1000),
    message          TEXT NOT NULL DEFAULT '' CHECK (length(message) <= 2000),
    completed_at     TEXT,
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE event_settings (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    class_name        TEXT NOT NULL DEFAULT 'Ma classe' CHECK (length(class_name) <= 80),
    event_name        TEXT NOT NULL DEFAULT 'Secret Santa' CHECK (length(event_name) <= 80),
    budget            TEXT NOT NULL DEFAULT '10 €' CHECK (length(budget) <= 40),
    exchange_date     TEXT NOT NULL DEFAULT '' CHECK (length(exchange_date) <= 40),
    welcome_message   TEXT NOT NULL DEFAULT 'Bienvenue dans le Secret Santa de ta classe !'
                      CHECK (length(welcome_message) <= 300),
    draw_completed_at TEXT,
    reveal_enabled    INTEGER NOT NULL DEFAULT 0 CHECK (reveal_enabled IN (0, 1)),
    -- on ne peut autoriser les révélations que si le tirage existe
    CHECK (reveal_enabled = 0 OR draw_completed_at IS NOT NULL)
);
INSERT INTO event_settings (id) VALUES (1);

CREATE TABLE assignments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    giver_id    INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    receiver_id INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    revealed_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    CHECK (giver_id <> receiver_id)
);

-- Les attributions ne sont créées que pendant le tirage, entre élèves, et ne changent plus.
CREATE TRIGGER assignments_only_after_draw BEFORE INSERT ON assignments
WHEN (SELECT draw_completed_at FROM event_settings WHERE id = 1) IS NULL
  OR (SELECT role FROM users WHERE id = NEW.giver_id) <> 'student'
  OR (SELECT role FROM users WHERE id = NEW.receiver_id) <> 'student'
BEGIN
    SELECT RAISE(ABORT, 'attribution invalide');
END;

CREATE TRIGGER assignments_immutable BEFORE UPDATE OF giver_id, receiver_id ON assignments
BEGIN
    SELECT RAISE(ABORT, 'les attributions sont définitives');
END;

CREATE TRIGGER assignments_no_delete BEFORE DELETE ON assignments
BEGIN
    SELECT RAISE(ABORT, 'les attributions sont définitives');
END;

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);

CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    actor_name TEXT NOT NULL DEFAULT '',
    action     TEXT NOT NULL,
    details    TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
