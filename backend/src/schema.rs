// @generated automatically by Diesel CLI.

diesel::table! {
    batches (id) {
        id -> Text,
        line_id -> Text,
        label -> Text,
        selection_json -> Text,
        stage_values -> Nullable<Text>,
        status -> Text,
        paused_reason -> Nullable<Text>,
        skip_if_generated -> Bool,
        cursor_key -> Nullable<Text>,
        cursor_shot_id -> Nullable<Text>,
        matched_total -> Nullable<Integer>,
        skipped_total -> Nullable<Integer>,
        est_tasks -> Nullable<Integer>,
        est_gpu_seconds -> Nullable<Integer>,
        est_disk_bytes -> Nullable<BigInt>,
        daily_task_cap -> Nullable<Integer>,
        window_start_minute -> Nullable<Integer>,
        window_end_minute -> Nullable<Integer>,
        disk_floor_bytes -> Nullable<BigInt>,
        max_outstanding_holds -> Nullable<Integer>,
        created_at -> Nullable<Timestamp>,
        finished_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    comfyui_workflows (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        workflow_json -> Text,
        inputs_json -> Nullable<Text>,
        outputs_json -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        contract_json -> Nullable<Text>,
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
        output_prefix -> Nullable<Text>,
        settle_until -> Nullable<Timestamp>,
        next_attempt_at -> Nullable<Timestamp>,
        source_mode -> Nullable<Text>,
        parameters -> Nullable<Text>,
        run_id -> Nullable<Text>,
        stage_idx -> Nullable<Integer>,
        parent_task_id -> Nullable<Text>,
        text_output -> Nullable<Text>,
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
        synthetic -> Bool,
        manifest_json -> Nullable<Text>,
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
    line_stages (id) {
        id -> Text,
        line_id -> Text,
        stage_idx -> Integer,
        workflow_id -> Text,
        text_overrides -> Nullable<Text>,
        parameters -> Nullable<Text>,
        vary -> Nullable<Text>,
        source_mode -> Nullable<Text>,
        keep_output -> Bool,
        created_at -> Nullable<Timestamp>,
        exposed -> Nullable<Text>,
        hold_for_review -> Bool,
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
    production_lines (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        updated_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    run_holds (id) {
        id -> Text,
        run_id -> Text,
        stage_idx -> Integer,
        verdict -> Text,
        reviewed_task_ids -> Text,
        kept_task_ids -> Text,
        note -> Nullable<Text>,
        decided_at -> Nullable<Timestamp>,
    }
}

diesel::table! {
    runs (id) {
        id -> Text,
        line_id -> Nullable<Text>,
        shot_id -> Text,
        label -> Text,
        status -> Text,
        stage_count -> Integer,
        error_message -> Nullable<Text>,
        created_at -> Nullable<Timestamp>,
        finished_at -> Nullable<Timestamp>,
        stage_values -> Nullable<Text>,
        held_at_stage -> Nullable<Integer>,
        batch_id -> Nullable<Text>,
    }
}

diesel::table! {
    saved_selections (id) {
        id -> Text,
        name -> Text,
        line_id -> Nullable<Text>,
        selection_json -> Text,
        caps_json -> Nullable<Text>,
        skip_if_generated -> Bool,
        created_at -> Nullable<Timestamp>,
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
        analysis_json -> Nullable<Text>,
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
        parameters -> Nullable<Text>,
    }
}

diesel::joinable!(enhancement_tasks -> comfyui_workflows (workflow_id));
diesel::joinable!(enhancement_tasks -> runs (run_id));
diesel::joinable!(enhancement_tasks -> shots (shot_id));
diesel::joinable!(faces -> files (file_id));
diesel::joinable!(faces -> people (person_id));
diesel::joinable!(files -> shots (shot_id));
diesel::joinable!(line_stages -> comfyui_workflows (workflow_id));
diesel::joinable!(line_stages -> production_lines (line_id));
diesel::joinable!(run_holds -> runs (run_id));
diesel::joinable!(runs -> batches (batch_id));
diesel::joinable!(runs -> production_lines (line_id));
diesel::joinable!(runs -> shots (shot_id));
diesel::joinable!(shots -> people (primary_person_id));
diesel::joinable!(video_keyframes -> files (video_file_id));
diesel::joinable!(workflow_presets -> comfyui_workflows (workflow_id));

diesel::allow_tables_to_appear_in_same_query!(
    batches,
    comfyui_workflows,
    enhancement_tasks,
    faces,
    files,
    ignored_merges,
    line_stages,
    people,
    production_lines,
    run_holds,
    runs,
    saved_selections,
    settings,
    shots,
    video_keyframes,
    workflow_presets,
);
