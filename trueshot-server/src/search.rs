use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, ReloadPolicy};
use std::sync::Arc;
use anyhow::Result;

pub struct SearchEngine {
    index: Index,
    reader: tantivy::IndexReader,
    schema: Schema,
}

impl SearchEngine {
    pub fn new() -> Result<Self> {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_text_field("tags", TEXT | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommit)
            .try_into()?;

        Ok(Self { index, reader, schema })
    }

    pub fn index_doc(&self, name: &str, tags: &str) -> Result<()> {
        let mut index_writer = self.index.writer(50_000_000)?;
        let name_field = self.schema.get_field("name").unwrap();
        let tags_field = self.schema.get_field("tags").unwrap();

        index_writer.add_document(doc!(
            name_field => name,
            tags_field => tags,
        ))?;

        index_writer.commit()?;
        Ok(())
    }

    pub fn search(&self, query_str: &str) -> Result<Vec<String>> {
        let searcher = self.reader.searcher();
        let name_field = self.schema.get_field("name").unwrap();
        let tags_field = self.schema.get_field("tags").unwrap();
        let query_parser = QueryParser::for_index(&self.index, vec![name_field, tags_field]);
        
        let query = query_parser.parse_query(query_str)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc = searcher.doc(doc_address)?;
            if let Some(val) = retrieved_doc.get_first(name_field) {
                if let Some(s) = val.as_text() {
                    results.push(s.to_string());
                }
            }
        }
        Ok(results)
    }
}
