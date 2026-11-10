-- Drop triggers that reference the shared updated_at helper, then drop the function.
-- Scoped to our helper name only (stricter than CASCADE, which would drop any dependency type).
DO $$
DECLARE r RECORD;
BEGIN
  FOR r IN
    SELECT t.tgname AS tgname, c.relname AS relname
    FROM pg_trigger t
    JOIN pg_class c ON c.oid = t.tgrelid
    JOIN pg_proc p ON p.oid = t.tgfoid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public'
      AND NOT t.tgisinternal
      AND p.proname = 'musehub_set_updated_at'
  LOOP
    EXECUTE format('DROP TRIGGER IF EXISTS %I ON public.%I', r.tgname, r.relname);
  END LOOP;
END $$;

DROP FUNCTION IF EXISTS musehub_set_updated_at();
