// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

fn directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!("kyomi-owner-lock-test-{}", Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn lock_is_proof_only_after_holder_drops_and_never_from_missing_file() {
    let path = directory();
    let holder = OwnerRecovery::create(&path).unwrap();
    let verifier = OwnerRecovery::create(&path).unwrap();
    let owner = holder.new_socket_owner();
    assert!(verifier.prove_dead(&owner).unwrap().is_none());
    let process_path = path.join(holder.process.to_string());
    drop(holder);
    assert!(verifier.prove_dead(&owner).unwrap().is_some());
    // Missing files never manufacture a death proof, even after a known exit.
    std::fs::remove_file(process_path).unwrap();
    assert!(verifier.prove_dead(&owner).unwrap().is_none());
    drop(verifier);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn foreign_legacy_malformed_and_replaced_files_fail_closed() {
    let path = directory();
    let foreign_path = directory();
    let holder = OwnerRecovery::create(&path).unwrap();
    let verifier = OwnerRecovery::create(&foreign_path).unwrap();
    let owner = holder.new_socket_owner();
    drop(holder);
    for value in [&owner, "legacy-uuid", "lock-v1:../bad:bad:1:2:bad"] {
        assert!(verifier.prove_dead(value).unwrap().is_none());
    }
    let verifier = OwnerRecovery::create(&path).unwrap();
    let process = owner.split(':').nth(2).unwrap();
    let lock_path = path.join(process);
    // Keep the original inode allocated so replacement cannot reuse it.
    let old_file = File::open(&lock_path).unwrap();
    std::fs::remove_file(&lock_path).unwrap();
    std::fs::write(&lock_path, process).unwrap();
    assert!(verifier.prove_dead(&owner).unwrap().is_none());
    drop(old_file);
    drop(verifier);
    std::fs::remove_dir_all(path).unwrap();
    std::fs::remove_dir_all(foreign_path).unwrap();
}

#[test]
fn process_lifetime_lock_survives_all_registry_reference_drops() {
    let path = directory();
    let process = OwnerRecovery::for_process(&path).unwrap();
    let owner = process.new_socket_owner();
    drop(process);
    let verifier = OwnerRecovery::create(&path).unwrap();
    assert!(verifier.prove_dead(&owner).unwrap().is_none());
    drop(verifier);
    // Intentionally retain this directory just as production retains lock files.
}

#[test]
fn corrupt_scope_fails_initialization_instead_of_creating_new_identity() {
    let path = directory();
    std::fs::write(path.join("scope"), "incomplete").unwrap();
    assert!(OwnerRecovery::create(&path).is_err());
    assert_eq!(
        std::fs::read_to_string(path.join("scope")).unwrap(),
        "incomplete"
    );
    std::fs::remove_dir_all(path).unwrap();
}
