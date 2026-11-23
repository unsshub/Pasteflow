-- 001_init.sql
CREATE TABLE spaces (
  id          INTEGER PRIMARY KEY,
  name        TEXT    NOT NULL UNIQUE COLLATE NOCASE,
  color       TEXT    NOT NULL DEFAULT '#8a8f98',
  icon        TEXT    NOT NULL DEFAULT 'folder-symbolic',
  position    INTEGER NOT NULL,
  is_default  INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0,1)),
  created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX spaces_one_default ON spaces(is_default) WHERE is_default = 1;

CREATE TABLE clips (
  id            INTEGER PRIMARY KEY,
  kind          TEXT    NOT NULL CHECK (kind IN
                 ('text','code','url','color','email','phone','number','image','files')),
  content_hash  BLOB    NOT NULL,
  title         TEXT,
  preview       TEXT    NOT NULL,
  search_text   TEXT    NOT NULL DEFAULT '',
  meta          TEXT,
  byte_size     INTEGER NOT NULL,
  space_id      INTEGER NOT NULL REFERENCES spaces(id) ON DELETE RESTRICT,
  pinned        INTEGER NOT NULL DEFAULT 0,
  selection     TEXT    NOT NULL DEFAULT 'CLIPBOARD' CHECK (selection IN ('CLIPBOARD','PRIMARY')),
  source_app    TEXT,
  created_at    INTEGER NOT NULL,
  last_used_at  INTEGER NOT NULL,
  copy_count    INTEGER NOT NULL DEFAULT 1,
  expires_at    INTEGER
);
CREATE UNIQUE INDEX clips_hash   ON clips(content_hash);
CREATE INDEX clips_recent        ON clips(pinned DESC, last_used_at DESC);
CREATE INDEX clips_space         ON clips(space_id, pinned DESC, last_used_at DESC);
CREATE INDEX clips_kind          ON clips(kind, last_used_at DESC);
CREATE INDEX clips_expiry        ON clips(expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE clip_formats (
  clip_id     INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  mime        TEXT    NOT NULL,
  inline_data BLOB,
  blob_key    TEXT,
  byte_size   INTEGER NOT NULL,
  PRIMARY KEY (clip_id, mime),
  CHECK ((inline_data IS NULL) <> (blob_key IS NULL))
) WITHOUT ROWID;

CREATE TABLE images (
  clip_id       INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
  width         INTEGER NOT NULL,
  height        INTEGER NOT NULL,
  original_mime TEXT    NOT NULL,
  thumb_key     TEXT
);

CREATE TABLE queue_items (
  id       INTEGER PRIMARY KEY,
  clip_id  INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  ord      INTEGER NOT NULL,
  added_at INTEGER NOT NULL
);
CREATE INDEX queue_ord ON queue_items(ord);

CREATE VIRTUAL TABLE clips_fts USING fts5(
  search_text, title,
  content='clips', content_rowid='id',
  tokenize='trigram'
);
INSERT INTO clips_fts(clips_fts, rank) VALUES('secure-delete', 1);

CREATE TRIGGER clips_ai AFTER INSERT ON clips BEGIN
  INSERT INTO clips_fts(rowid, search_text, title) VALUES (new.id, new.search_text, new.title);
END;
CREATE TRIGGER clips_ad AFTER DELETE ON clips BEGIN
  INSERT INTO clips_fts(clips_fts, rowid, search_text, title) VALUES('delete', old.id, old.search_text, old.title);
END;
CREATE TRIGGER clips_au AFTER UPDATE OF search_text, title ON clips BEGIN
  INSERT INTO clips_fts(clips_fts, rowid, search_text, title) VALUES('delete', old.id, old.search_text, old.title);
  INSERT INTO clips_fts(rowid, search_text, title) VALUES (new.id, new.search_text, new.title);
END;

INSERT INTO spaces(name,color,icon,position,is_default,created_at) VALUES
 ('General','#8a8f98','view-grid-symbolic',0,1,0),
 ('Development','#3584e4','utilities-terminal-symbolic',1,0,0),
 ('Design','#c061cb','applications-graphics-symbolic',2,0,0),
 ('Personal','#33d17a','user-home-symbolic',3,0,0),
 ('Business','#f6d32d','x-office-document-symbolic',4,0,0);
