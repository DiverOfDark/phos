-- Baseline migration: captures the current schema state.
-- Uses CREATE TABLE IF NOT EXISTS so it's safe to run on existing databases.

CREATE TABLE IF NOT EXISTS people (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT,
    thumbnail_face_id TEXT,
    representative_embedding BLOB,
    folder_name TEXT UNIQUE,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS shots (
    id TEXT PRIMARY KEY NOT NULL,
    main_file_id TEXT,
    timestamp DATETIME,
    width INTEGER,
    height INTEGER,
    latitude REAL,
    longitude REAL,
    primary_person_id TEXT REFERENCES people(id),
    folder_number INTEGER,
    review_status TEXT DEFAULT 'pending',
    description TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY NOT NULL,
    shot_id TEXT NOT NULL REFERENCES shots(id),
    path TEXT NOT NULL UNIQUE,
    hash TEXT NOT NULL,
    mime_type TEXT,
    file_size INTEGER,
    is_original BOOLEAN DEFAULT 0,
    visual_embedding BLOB,
    source_workflow_id TEXT,
    source_text_overrides TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS faces (
    id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL REFERENCES files(id),
    person_id TEXT REFERENCES people(id),
    box_x1 REAL,
    box_y1 REAL,
    box_x2 REAL,
    box_y2 REAL,
    embedding BLOB,
    thumbnail_path TEXT,
    score REAL
);

CREATE TABLE IF NOT EXISTS video_keyframes (
    id TEXT PRIMARY KEY NOT NULL,
    video_file_id TEXT NOT NULL REFERENCES files(id),
    timestamp_ms INTEGER,
    path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS comfyui_workflows (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    workflow_json TEXT NOT NULL,
    inputs_json TEXT,
    outputs_json TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS enhancement_tasks (
    id TEXT PRIMARY KEY NOT NULL,
    shot_id TEXT NOT NULL REFERENCES shots(id),
    workflow_id TEXT NOT NULL REFERENCES comfyui_workflows(id),
    status TEXT NOT NULL DEFAULT 'pending',
    comfyui_prompt_id TEXT,
    text_overrides TEXT DEFAULT '{}',
    source_file_id TEXT REFERENCES files(id),
    output_file_id TEXT REFERENCES files(id),
    error_message TEXT,
    retry_count INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    started_at DATETIME,
    completed_at DATETIME
);

CREATE TABLE IF NOT EXISTS workflow_presets (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_id TEXT NOT NULL REFERENCES comfyui_workflows(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    text_overrides TEXT NOT NULL DEFAULT '{}',
    sort_order INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS ignored_merges (
    shot_id_1 TEXT NOT NULL,
    shot_id_2 TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (shot_id_1, shot_id_2)
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_faces_person_id ON faces(person_id);
CREATE INDEX IF NOT EXISTS idx_faces_file_id ON faces(file_id);
CREATE INDEX IF NOT EXISTS idx_files_shot_id ON files(shot_id);
CREATE INDEX IF NOT EXISTS idx_shots_primary_person_id ON shots(primary_person_id);
CREATE INDEX IF NOT EXISTS idx_shots_timestamp ON shots(timestamp);

-- Triggers for auto-updating updated_at
CREATE TRIGGER IF NOT EXISTS people_updated_at AFTER UPDATE ON people
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
BEGIN
    UPDATE people SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS shots_updated_at AFTER UPDATE ON shots
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
BEGIN
    UPDATE shots SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS files_updated_at AFTER UPDATE ON files
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
BEGIN
    UPDATE files SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
