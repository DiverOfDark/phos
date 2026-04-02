// @generated automatically by Diesel CLI.

diesel::table! {
    comfyui_workflows (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        workflow_json -> Text,
        inputs_json -> Nullable<Text>,
        outputs_json -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    enhancement_tasks (id) {
        id -> Text,
        shot_id -> Text,
        workflow_id -> Text,
        status -> Text,
        comfyui_prompt_id -> Nullable<Text>,
        text_overrides -> Nullable<Text>,
        source_file_id -> Nullable<Text>,
        output_file_id -> Nullable<Text>,
        error_message -> Nullable<Text>,
        retry_count -> Nullable<Integer>,
        created_at -> Nullable<Timestamp>,
        started_at -> Nullable<Timestamp>,
        completed_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    faces (id) {
        id -> Text,
        file_id -> Text,
        person_id -> Nullable<Text>,
        box_x1 -> Nullable<Float>,
        box_y1 -> Nullable<Float>,
        box_x2 -> Nullable<Float>,
        box_y2 -> Nullable<Float>,
        embedding -> Nullable<Binary>,
        thumbnail_path -> Nullable<Text>,
        score -> Nullable<Float>,
    }
}

diesel::table! {
    files (id) {
        id -> Text,
        shot_id -> Text,
        path -> Text,
        hash -> Text,
        mime_type -> Nullable<Text>,
        file_size -> Nullable<Integer>,
        is_original -> Nullable<Bool>,
        visual_embedding -> Nullable<Binary>,
        source_workflow_id -> Nullable<Text>,
        source_text_overrides -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        updated_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    ignored_merges (shot_id_1, shot_id_2) {
        shot_id_1 -> Text,
        shot_id_2 -> Text,
        created_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    people (id) {
        id -> Text,
        name -> Nullable<Text>,
        thumbnail_face_id -> Nullable<Text>,
        representative_embedding -> Nullable<Binary>,
        folder_name -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        updated_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    settings (key) {
        key -> Text,
        value -> Text,
    }
}

diesel::table! {
    shots (id) {
        id -> Text,
        main_file_id -> Nullable<Text>,
        timestamp -> Nullable<Timestamp>,
        width -> Nullable<Integer>,
        height -> Nullable<Integer>,
        latitude -> Nullable<Float>,
        longitude -> Nullable<Float>,
        primary_person_id -> Nullable<Text>,
        folder_number -> Nullable<Integer>,
        review_status -> Nullable<Text>,
        description -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        updated_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    video_keyframes (id) {
        id -> Text,
        video_file_id -> Text,
        timestamp_ms -> Nullable<Integer>,
        path -> Text,
    }
}

diesel::table! {
    workflow_presets (id) {
        id -> Text,
        workflow_id -> Text,
        name -> Text,
        text_overrides -> Text,
        sort_order -> Nullable<Integer>,
        created_at -> Nullable<Timestamp>,
    }
}

diesel::joinable!(enhancement_tasks -> comfyui_workflows (workflow_id));
diesel::joinable!(enhancement_tasks -> shots (shot_id));
diesel::joinable!(faces -> files (file_id));
diesel::joinable!(faces -> people (person_id));
diesel::joinable!(files -> shots (shot_id));
diesel::joinable!(shots -> people (primary_person_id));
diesel::joinable!(video_keyframes -> files (video_file_id));
diesel::joinable!(workflow_presets -> comfyui_workflows (workflow_id));

diesel::allow_tables_to_appear_in_same_query!(
    comfyui_workflows,
    enhancement_tasks,
    faces,
    files,
    ignored_merges,
    people,
    settings,
    shots,
    video_keyframes,
    workflow_presets,
);
