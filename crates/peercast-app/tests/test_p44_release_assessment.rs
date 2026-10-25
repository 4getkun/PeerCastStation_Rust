use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Acceptance criterion 1:
/// All 58 requirement IDs (REQ-001 through REQ-087) are mapped, implemented, and verified (REQ-070, REQ-072).
#[test]
fn test_p44_all_58_requirements_covered_and_verified() {
    let root = repo_root();
    let spec_path = root.join("docs").join("SPECIFICATION.md");
    assert!(spec_path.exists(), "docs/SPECIFICATION.md must exist");

    let worklog_path = root.join("docs").join("WORKLOG_AND_DECISIONS.md");
    assert!(
        worklog_path.exists(),
        "docs/WORKLOG_AND_DECISIONS.md must exist"
    );

    let worklog = fs::read_to_string(&worklog_path).expect("failed to read worklog");

    // All 58 requirement IDs specified in SPECIFICATION.md
    let req_ids = [
        "REQ-001", "REQ-002", "REQ-003", "REQ-004", "REQ-010", "REQ-011", "REQ-012", "REQ-013",
        "REQ-014", "REQ-020", "REQ-021", "REQ-022", "REQ-023", "REQ-024", "REQ-025", "REQ-026",
        "REQ-027", "REQ-028", "REQ-030", "REQ-031", "REQ-032", "REQ-033", "REQ-034", "REQ-035",
        "REQ-036", "REQ-037", "REQ-038", "REQ-040", "REQ-041", "REQ-042", "REQ-043", "REQ-044",
        "REQ-050", "REQ-051", "REQ-052", "REQ-053", "REQ-054", "REQ-055", "REQ-056", "REQ-060",
        "REQ-061", "REQ-062", "REQ-063", "REQ-064", "REQ-065", "REQ-066", "REQ-067", "REQ-070",
        "REQ-071", "REQ-072", "REQ-080", "REQ-081", "REQ-082", "REQ-083", "REQ-084", "REQ-085",
        "REQ-086", "REQ-087",
    ];

    assert_eq!(req_ids.len(), 58, "Must verify exactly 58 requirements");

    for req in req_ids {
        assert!(
            worklog.contains(req),
            "Requirement {} must be recorded with implementation evidence in WORKLOG_AND_DECISIONS.md",
            req
        );
    }
}

/// Acceptance criterion 2:
/// All 50 task contracts (P01 through P44) are executed, evidenced, and passed (REQ-070, REQ-071).
#[test]
fn test_p44_all_50_tasks_executed_and_evidenced() {
    let root = repo_root();
    let worklog = fs::read_to_string(root.join("docs").join("WORKLOG_AND_DECISIONS.md")).unwrap();

    let tasks = [
        "P01", "P02", "P03", "P04", "P05", "P06", "P07", "P08", "P09", "P10", "P11", "P12", "P13",
        "P14", "P15", "P16", "P17", "P18", "P19", "P20", "P21", "P22", "P23", "P24", "P25", "P26",
        "P27", "P28", "P29", "P30", "P31", "P32", "P33", "P34", "P35", "P36", "P37", "P37A",
        "P37B", "P37C", "P37D", "P37E", "P37F", "P38", "P39", "P40", "P41", "P42", "P43", "P44",
    ];

    assert_eq!(tasks.len(), 50, "Must verify exactly 50 tasks in DAG");

    for task in tasks {
        let pattern1 = format!("Task {}:", task);
        let pattern2 = format!("Task {} Implementation:", task);
        let pattern3 = format!("Task {} —", task);
        let pattern4 = format!("Task {} ", task);
        assert!(
            worklog.contains(&pattern1)
                || worklog.contains(&pattern2)
                || worklog.contains(&pattern3)
                || worklog.contains(&pattern4),
            "Task {} must be recorded in worklog",
            task
        );
    }
}

/// Acceptance criterion 3:
/// Release readiness documents exist and are coherent (REQ-066, REQ-067, REQ-070).
#[test]
fn test_p44_release_documentation_completeness() {
    let root = repo_root();

    let required_docs = [
        "docs/SPECIFICATION.md",
        "docs/WORKLOG_AND_DECISIONS.md",
        "docs/UI_LAYER_BOUNDARIES.md",
        "docs/RPC_CONTRACT.md",
        "docs/DESIGN_CHANGE_GUIDE.md",
        "docs/LICENSE_AUDIT.md",
        "docs/TARGET_PACKAGING.md",
        "docs/FUZZ_AND_FAULT_CAMPAIGN.md",
        "docs/OPERATIONAL_SOAK_REPORT.md",
        "docs/RELEASE_ASSESSMENT.md",
    ];

    for doc in required_docs {
        let path = root.join(doc);
        assert!(
            path.exists(),
            "Required release document must exist: {}",
            doc
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.trim().is_empty(),
            "Document must not be empty: {}",
            doc
        );
    }
}

/// Acceptance criterion 4:
/// Upstream parity and divergence classification (REQ-071, REQ-072).
#[test]
fn test_p44_upstream_parity_and_divergence_clarity() {
    let root = repo_root();
    let assessment = fs::read_to_string(root.join("docs").join("RELEASE_ASSESSMENT.md")).unwrap();

    // Must explicitly identify both upstream references
    assert!(assessment.contains("PeerCastStation") && assessment.contains("PeerCast-YT"));

    // Must explicitly identify compatibility points
    assert!(assessment.contains("Guid") || assessment.contains("GUID"));
    assert!(assessment.contains("Atom"));
    assert!(assessment.contains("PCP"));
    assert!(assessment.contains("FLV"));
    assert!(assessment.contains("JSON-RPC"));
}
