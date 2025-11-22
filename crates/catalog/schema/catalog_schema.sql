PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS catalog_metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    last_opened TEXT
);

CREATE TRIGGER IF NOT EXISTS catalog_metadata_touch_updated_at
AFTER UPDATE ON catalog_metadata
FOR EACH ROW
BEGIN
    UPDATE catalog_metadata
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_path ON folders(path);

CREATE TRIGGER IF NOT EXISTS folders_touch_updated_at
AFTER UPDATE ON folders
FOR EACH ROW
BEGIN
    UPDATE folders
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS images (
    id INTEGER PRIMARY KEY,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    original_path TEXT NOT NULL UNIQUE,
    sidecar_path TEXT,
    sidecar_hash TEXT,
    filesize INTEGER,
    file_hash TEXT,
    file_modified_at TEXT,
    imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    captured_at TEXT,
    camera_make TEXT,
    camera_model TEXT,
    lens_model TEXT,
    focal_length REAL,
    aperture REAL,
    shutter_speed REAL,
    iso INTEGER,
    orientation INTEGER,
    gps_latitude REAL,
    gps_longitude REAL,
    gps_altitude REAL,
    rating INTEGER CHECK (rating BETWEEN 0 AND 5),
    flag TEXT CHECK (flag IN ('picked','rejected') OR flag IS NULL),
    color_label TEXT CHECK (
        color_label IN ('red','yellow','green','blue','purple','orange','teal')
        OR color_label IS NULL
    ),
    metadata_json TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_images_folder_id ON images(folder_id);
CREATE INDEX IF NOT EXISTS idx_images_captured_at ON images(captured_at);
CREATE INDEX IF NOT EXISTS idx_images_file_hash ON images(file_hash);

CREATE TRIGGER IF NOT EXISTS images_touch_updated_at
AFTER UPDATE ON images
FOR EACH ROW
BEGIN
    UPDATE images
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS edits (
    id INTEGER PRIMARY KEY,
    image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    exposure REAL,
    contrast REAL,
    highlights REAL,
    shadows REAL,
    whites REAL,
    blacks REAL,
    vibrance REAL,
    saturation REAL,
    temperature REAL,
    tint REAL,
    texture REAL,
    clarity REAL,
    dehaze REAL,
    parametric_curve_json TEXT CHECK (parametric_curve_json IS NULL OR json_valid(parametric_curve_json)),
    color_grading_json TEXT CHECK (color_grading_json IS NULL OR json_valid(color_grading_json)),
    crop_json TEXT CHECK (crop_json IS NULL OR json_valid(crop_json)),
    masking_json TEXT CHECK (masking_json IS NULL OR json_valid(masking_json)),
    updated_at TEXT,
    FOREIGN KEY (image_id) REFERENCES images(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_edits_image_id ON edits(image_id);

CREATE TABLE IF NOT EXISTS edit_history (
    id INTEGER PRIMARY KEY,
    image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    edits_json TEXT NOT NULL CHECK (json_valid(edits_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_edit_history_image_id ON edit_history(image_id);

CREATE TABLE IF NOT EXISTS keywords (
    id INTEGER PRIMARY KEY,
    keyword TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS image_keywords (
    image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    keyword_id INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
    assigned_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (image_id, keyword_id)
);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id INTEGER REFERENCES collections(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TRIGGER IF NOT EXISTS collections_touch_updated_at
AFTER UPDATE ON collections
FOR EACH ROW
BEGIN
    UPDATE collections
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS collection_images (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (collection_id, image_id)
);

CREATE INDEX IF NOT EXISTS idx_collection_images_collection_id
    ON collection_images(collection_id, position);

CREATE TABLE IF NOT EXISTS thumbnails (
    image_id INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    thumb_256 BLOB,
    thumb_1024 BLOB,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS previews (
    image_id INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    preview_blob BLOB,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TRIGGER IF NOT EXISTS thumbnails_touch_updated_at
AFTER UPDATE ON thumbnails
FOR EACH ROW
BEGIN
    UPDATE thumbnails
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE image_id = NEW.image_id;
END;

CREATE TRIGGER IF NOT EXISTS previews_touch_updated_at
AFTER UPDATE ON previews
FOR EACH ROW
BEGIN
    UPDATE previews
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE image_id = NEW.image_id;
END;

INSERT INTO catalog_metadata (id, schema_version, created_at, updated_at, last_opened)
VALUES (
    1,
    1,
    strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    NULL
)
ON CONFLICT(id) DO NOTHING;

PRAGMA user_version = 1;
