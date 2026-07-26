#!/bin/bash
# Generate Documentation Site
echo "Generating RustDocs..."
cargo doc --no-deps --document-private-items

echo "Generating TypeDocs..."
cd trueshot-dashboard
npm install typedoc --save-dev
npx typedoc src/index.tsx --out ../docs/frontend

echo "Packaging Site..."
mkdir -p ../docs/site
cp -r ../target/doc ../docs/site/api
cp -r ../docs/frontend ../docs/site/ui
echo "Documentation Built at docs/site"
