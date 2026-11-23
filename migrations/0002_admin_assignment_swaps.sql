-- Allow narrowly scoped administrator swaps after the draw.
-- Preserve uniqueness and immutability for all ordinary operations.
ALTER TABLE assignments DROP CONSTRAINT assignments_receiver_id_key;
ALTER TABLE assignments ADD CONSTRAINT assignments_receiver_id_key
    UNIQUE (receiver_id) DEFERRABLE INITIALLY IMMEDIATE;

CREATE OR REPLACE FUNCTION assignments_forbid_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND current_setting('santa.allow_assignment_swap', true) = 'on'
       AND OLD.giver_id = NEW.giver_id
       AND OLD.revealed_at IS NULL
       AND NEW.revealed_at IS NULL THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'les attributions sont définitives';
END;
$$;
