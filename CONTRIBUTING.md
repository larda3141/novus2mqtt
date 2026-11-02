# Contributing to novus2mqtt

Thank you for your interest in contributing! We welcome contributions from the community.

## Getting Started

### Prerequisites
- Rust toolchain (stable, 2021 edition)
- Git
- Basic knowledge of Rust and MQTT

### Development Setup

```bash
# Clone the repository
git clone https://github.com/larda3141/novus2mqtt.git
cd novus2mqtt

# Build the project
cargo build

# Run tests
cargo test

# Run the application
cargo run
```

## Code Style and Quality

We follow standard Rust conventions enforced by `rustfmt` and `clippy`.

### Before Submitting Code

1. **Format your code**:
   ```bash
   cargo fmt
   ```

2. **Run linting**:
   ```bash
   cargo clippy -- -D warnings
   ```

3. **Run tests**:
   ```bash
   cargo test
   ```

4. **Check everything**:
   ```bash
   cargo fmt --check && cargo clippy -- -D warnings && cargo test
   ```

5. **Audit dependencies** (CI also runs this):
   ```bash
   cargo install cargo-audit  # one-time
   cargo audit
   ```

## Making Changes

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/your-feature-name
   # or for bug fixes:
   git checkout -b fix/your-bug-fix
   ```

2. **Make your changes** with clear, descriptive commits

3. **Add or update tests** for your changes

4. **Update documentation** if needed (README, ARCHITECTURE.md, etc.)

5. **Update CHANGELOG.md** with your changes

## Pull Request Process

1. Push your branch to your fork
2. Create a Pull Request with a clear description of changes
3. Reference any related issues
4. Ensure CI checks pass (build, test, clippy, fmt)
5. Request review from maintainers

## What to Contribute

### Great for Beginners
- Documentation improvements
- Bug fixes with tests
- Error message improvements

### Intermediate
- New register support in `config/registers.yaml`
- Test coverage expansion
- Home Assistant entity improvements

### Advanced
- Protocol enhancements
- Performance optimizations
- New integration features

## Bug Reports

Create an issue with:
- Clear title and description
- Steps to reproduce
- Expected vs. actual behavior
- Environment details (Rust version, OS, etc.)
- Relevant logs or error messages

## Feature Requests

Create an issue with:
- Clear description of the desired feature
- Use case and motivation
- Possible implementation approach (optional)

## Licensing

By contributing, you agree that your contributions will be licensed under the MIT License.
