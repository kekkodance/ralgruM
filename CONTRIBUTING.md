# Contributing

Public pull requests run the stub-backend CI workflow. It checks formatting, Cargo metadata, compilation, tests, and Clippy without initializing the private submodule.

The full Windows application build is a separate workflow because it needs read access to kekkodance/ralgrum-murglar-private. Configure a repository secret named MURGLAR_PRIVATE_REPO_TOKEN with read access to that private repository before running it.

The Cargo package version remains semantic versioning. Full-build artifacts carry the parent commit SHA in their filename and in build-info.txt, together with the private backend commit SHA. This keeps Cargo metadata valid while making every artifact traceable to exact source commits.

To update the private backend:

    Set-Location src/murglar_backend/implementation
    git pull origin main
    # make and test private backend changes
    git add .
    git commit -m "Update private backend"
    git push origin main
    Set-Location ../..
    git add src/murglar_backend/implementation
    git commit -m "Update Murglar backend submodule"
    git push

The public stub can be selected locally with:

    $env:RALGRUM_MURGLAR_PRIVATE_DIR = 'target/no-private-backend'
    cargo test --locked --all-targets
    Remove-Item Env:RALGRUM_MURGLAR_PRIVATE_DIR
