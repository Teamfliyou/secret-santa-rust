-- Secret Santa : schéma initial (PostgreSQL)

-- Horodatage ISO 8601 en UTC, stocké en texte (ex. 2026-10-20T12:30:00Z).
CREATE FUNCTION iso_now() RETURNS TEXT
LANGUAGE SQL STABLE AS $$
    SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
$$;

CREATE TABLE users (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username      TEXT NOT NULL CHECK (char_length(username) BETWEEN 2 AND 64),
    first_name    TEXT NOT NULL CHECK (char_length(btrim(first_name)) BETWEEN 1 AND 60),
    last_name     TEXT NOT NULL CHECK (char_length(btrim(last_name)) BETWEEN 1 AND 60),
    -- chaîne vide = mot de passe pas encore choisi par l'élève
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'student' CHECK (role IN ('student', 'admin')),
    created_at    TEXT NOT NULL DEFAULT iso_now(),
    updated_at    TEXT NOT NULL DEFAULT iso_now()
);
-- identifiants insensibles à la casse
CREATE UNIQUE INDEX users_username_lower ON users (lower(username));

CREATE TABLE preferences (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id           BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    interests         TEXT NOT NULL DEFAULT '' CHECK (char_length(interests) <= 1000),
    hobbies           TEXT NOT NULL DEFAULT '' CHECK (char_length(hobbies) <= 1000),
    gift_types        TEXT NOT NULL DEFAULT '' CHECK (char_length(gift_types) <= 1000),
    favorite_snacks   TEXT NOT NULL DEFAULT '' CHECK (char_length(favorite_snacks) <= 1000),
    favorite_colors   TEXT NOT NULL DEFAULT '' CHECK (char_length(favorite_colors) <= 1000),
    wishlist          TEXT NOT NULL DEFAULT '' CHECK (char_length(wishlist) <= 1000),
    dislikes          TEXT NOT NULL DEFAULT '' CHECK (char_length(dislikes) <= 1000),
    food_restrictions TEXT NOT NULL DEFAULT '' CHECK (char_length(food_restrictions) <= 1000),
    message           TEXT NOT NULL DEFAULT '' CHECK (char_length(message) <= 2000),
    completed_at      TEXT,
    updated_at        TEXT NOT NULL DEFAULT iso_now()
);

CREATE TABLE event_settings (
    id                BIGINT PRIMARY KEY CHECK (id = 1),
    class_name        TEXT NOT NULL DEFAULT 'Ma classe' CHECK (char_length(class_name) <= 80),
    event_name        TEXT NOT NULL DEFAULT 'Secret Santa' CHECK (char_length(event_name) <= 80),
    budget            TEXT NOT NULL DEFAULT '10 €' CHECK (char_length(budget) <= 40),
    exchange_date     TEXT NOT NULL DEFAULT '' CHECK (char_length(exchange_date) <= 40),
    welcome_message   TEXT NOT NULL DEFAULT 'Bienvenue dans le Secret Santa de ta classe !'
                      CHECK (char_length(welcome_message) <= 300),
    draw_completed_at TEXT,
    reveal_enabled    BOOLEAN NOT NULL DEFAULT FALSE,
    -- on ne peut autoriser les révélations que si le tirage existe
    CHECK (NOT reveal_enabled OR draw_completed_at IS NOT NULL)
);
INSERT INTO event_settings (id) VALUES (1);

CREATE TABLE assignments (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    giver_id    BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    receiver_id BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    revealed_at TEXT,
    created_at  TEXT NOT NULL DEFAULT iso_now(),
    CHECK (giver_id <> receiver_id)
);

-- Les attributions ne sont créées que pendant le tirage, entre élèves, et ne changent plus.
CREATE FUNCTION assignments_check_insert() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF (SELECT draw_completed_at FROM event_settings WHERE id = 1) IS NULL
       OR (SELECT role FROM users WHERE id = NEW.giver_id) IS DISTINCT FROM 'student'
       OR (SELECT role FROM users WHERE id = NEW.receiver_id) IS DISTINCT FROM 'student' THEN
        RAISE EXCEPTION 'attribution invalide';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER assignments_only_after_draw BEFORE INSERT ON assignments
FOR EACH ROW EXECUTE FUNCTION assignments_check_insert();

CREATE FUNCTION assignments_forbid_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'les attributions sont définitives';
END;
$$;

CREATE TRIGGER assignments_immutable BEFORE UPDATE OF giver_id, receiver_id ON assignments
FOR EACH ROW EXECUTE FUNCTION assignments_forbid_change();

CREATE TRIGGER assignments_no_delete BEFORE DELETE ON assignments
FOR EACH ROW EXECUTE FUNCTION assignments_forbid_change();

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);

CREATE TABLE audit_log (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    actor_name TEXT NOT NULL DEFAULT '',
    action     TEXT NOT NULL,
    details    TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT iso_now()
);
