use super::*;
use serde_json::json;

const MEDIA: &str = "0199d3a4-1b2c-7d3e-8f40-123456789abc";

fn v1(content: Value) -> Value {
    json!({"type": "doc", "version": 1, "content": content})
}

fn doc(content: Value) -> Value {
    json!({"type": "doc", "version": 2, "content": content})
}

fn v3(content: Value) -> Value {
    json!({"type": "doc", "version": 3, "content": content})
}

fn text(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

fn para(inline: Value) -> Value {
    json!({"type": "paragraph", "content": [inline]})
}

fn cell(kind: &str, t: &str) -> Value {
    json!({"type": kind, "content": [para(text(t))]})
}

/// A `rows` × `columns` table, the first row a header row.
fn table(rows: usize, columns: usize) -> Value {
    let row = |kind: &str| json!({"type": "tableRow", "content": (0..columns).map(|c| cell(kind, &c.to_string())).collect::<Vec<_>>()});
    let mut content = vec![row("tableHeader")];
    content.extend((1..rows).map(|_| row("tableCell")));
    json!({"type": "table", "content": content})
}

fn embed(provider: &str, id: &str) -> Value {
    json!({"type": "embed", "attrs": {"provider": provider, "id": id}})
}

#[test]
fn valid_documents_round_trip_to_escaped_html() {
    let value = v1(json!([
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Title <script>"}]},
        {"type": "paragraph", "content": [
            {"type": "text", "text": "a \"b\" & 'c'", "marks": [{"type": "bold"}, {"type": "link", "attrs": {"href": "https://x.test/?a=1&b=\"2\""}}]},
            {"type": "hardBreak"}
        ]},
        {"type": "bulletList", "content": [{"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "i"}]}]}]},
        {"type": "codeBlock", "content": [{"type": "text", "text": "<b>x</b>"}]},
        {"type": "image", "attrs": {"media": MEDIA, "alt": "\" onerror=\"x"}},
        {"type": "horizontalRule"}
    ]));
    let parsed = parse(&value).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(media_ids(&parsed), vec![uuid::Uuid::parse_str(MEDIA).unwrap()]);
    let html = to_html(&parsed, &|id| Some(format!("/media/{id}")));
    assert_eq!(
        html,
        "<h2 id=\"title-script\">Title &lt;script&gt;</h2><p><a href=\"https://x.test/?a=1&amp;b=&quot;2&quot;\" rel=\"noopener noreferrer nofollow\">\
         <strong>a &quot;b&quot; &amp; &#39;c&#39;</strong></a><br></p><ul><li><p>i</p></li></ul>\
         <pre><code>&lt;b&gt;x&lt;/b&gt;</code></pre><figure><img src=\"/media/0199d3a4-1b2c-7d3e-8f40-123456789abc\" alt=\"&quot; onerror=&quot;x\"></figure><hr>"
    );
    assert!(!to_html(&parsed, &|_| None).contains("<img"));
}

#[test]
fn version_1_is_read_as_it_is_and_is_a_subset_of_version_2() {
    let content = json!([
        {"type": "heading", "attrs": {"level": 3}, "content": [text("A")]},
        {"type": "image", "attrs": {"media": MEDIA, "alt": "x"}},
        {"type": "blockquote", "content": [para(text("q"))]}
    ]);
    let old = parse(&v1(content.clone())).unwrap();
    let new = parse(&doc(content.clone())).unwrap();
    assert_eq!((old.version, new.version), (1, 2));
    assert_eq!(old.content, new.content);
    // `upgrade_value` changes the version only, 1 to 2; never to 3 (the lowest
    // version a document needs, so 0.10 to 0.12 keep reading it).
    assert_eq!(upgrade_value(&v1(content.clone())), doc(content.clone()));
    assert_eq!(parse(&upgrade_value(&v1(content.clone()))).unwrap(), new);
    for same in [
        doc(content.clone()),
        v3(content),
        json!("<p>x</p>"),
        json!({"type": "doc", "version": 4}),
        json!({"type": "x", "version": 1}),
    ] {
        assert_eq!(upgrade_value(&same), same);
    }
    // What only version 2 has is refused in version 1.
    for v2_only in [
        table(2, 2),
        embed("youtube", "dQw4w9WgXcQ"),
        json!({"type": "callout", "attrs": {"tone": "info"}, "content": [para(text("x"))]}),
        json!({"type": "image", "attrs": {"media": MEDIA, "caption": "c"}}),
        json!({"type": "image", "attrs": {"media": MEDIA, "width": "normal"}}),
    ] {
        let err = parse(&v1(json!([v2_only.clone()]))).unwrap_err();
        assert!(err.0.contains("needs document version 2"), "{v2_only}: {err}");
        assert!(parse(&doc(json!([v2_only.clone()]))).is_ok(), "{v2_only}");
        let issue = rustcms_core::i18n::Issue::of(&err.0);
        assert_eq!(issue.code, "document_needs_version", "{err}");
    }
}

#[test]
fn anything_else_is_refused() {
    let link = |href: &str| json!({"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": href}}]});
    for bad in [
        json!({"type": "doc", "content": []}),
        json!({"type": "doc", "version": 0, "content": []}),
        json!({"type": "doc", "version": 4, "content": []}),
        json!("<p>html</p>"),
        doc(json!([{"type": "script", "content": []}])),
        doc(json!([{"type": "paragraph", "attrs": {"style": "x"}, "content": []}])),
        doc(json!([para(link("javascript:alert(1)"))])),
        doc(json!([para(link("JavaScript:alert(1)"))])),
        doc(json!([para(link("data:text/html,x"))])),
        doc(json!([para(link("https://a b"))])),
        doc(json!([para(
            json!({"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": "https://x", "target": "_blank"}}]})
        )])),
        doc(json!([para(json!({"type": "text", "text": "x", "marks": [{"type": "underline"}]}))])),
        doc(json!([para(json!({"type": "text", "text": "", "marks": []}))])),
        doc(json!([para(json!({"type": "text", "text": "x", "onclick": "y"}))])),
        doc(json!([{"type": "heading", "attrs": {"level": 1}, "content": [text("x")]}])),
        doc(json!([{"type": "heading", "attrs": {"level": 2, "id": "x"}, "content": [text("x")]}])),
        doc(json!([{"type": "image", "attrs": {"src": "https://evil.test/x.png"}}])),
        doc(json!([{"type": "image", "attrs": {"media": "not-a-uuid"}}])),
        doc(json!([{"type": "bulletList", "content": [para(text("x"))]}])),
        doc(json!([{"type": "codeBlock", "content": [{"type": "text", "text": "x", "marks": [{"type": "bold"}]}]}])),
        doc(json!([para(json!({"type": "text", "text": "x", "marks": [{"type": "bold"}, {"type": "bold"}]}))])),
        doc(json!([para(json!({"type": "image", "attrs": {"media": MEDIA}}))])),
        doc(json!([{"type": "listItem", "content": [para(text("x"))]}])),
        json!({"type": "doc", "version": 2, "content": [], "marks": [{"type": "evil", "attrs": {"x": 1}}]}),
        // Images (version 2 attributes).
        doc(json!([{"type": "image", "attrs": {"media": MEDIA, "width": "huge"}}])),
        doc(json!([{"type": "image", "attrs": {"media": MEDIA, "width": 800}}])),
        doc(json!([{"type": "image", "attrs": {"media": MEDIA, "caption": {"html": "<b>"}}}])),
        doc(json!([{"type": "image", "attrs": {"media": MEDIA, "caption": "x".repeat(MAX_CAPTION + 1)}}])),
        doc(json!([{"type": "image", "attrs": {"media": MEDIA, "title": "x"}}])),
        // Embeds: allowed providers, their id formats, nothing else.
        doc(json!([embed("dailymotion", "x7tgad0")])),
        doc(json!([embed("YouTube", "dQw4w9WgXcQ")])),
        doc(json!([embed("youtube", "dQw4w9WgXc")])),
        doc(json!([embed("youtube", "dQw4w9WgXcQ\"><script>")])),
        doc(json!([embed("youtube", "https://www.youtube.com/watch?v=dQw4w9WgXcQ")])),
        doc(json!([embed("vimeo", "76979871a")])),
        doc(json!([embed("vimeo", "")])),
        doc(json!([embed("x", "1460323737035677698")])),
        doc(json!([embed("twitter", "1460323737035677698")])),
        doc(json!([embed("vimeo", "0123")])),
        doc(json!([embed("vimeo", "1".repeat(16).as_str())])),
        doc(json!([{"type": "embed", "attrs": {"provider": "youtube"}}])),
        doc(json!([{"type": "embed", "attrs": {"provider": "youtube", "id": "dQw4w9WgXcQ", "html": "<iframe>"}}])),
        doc(json!([{"type": "embed", "attrs": {"provider": "youtube", "id": "dQw4w9WgXcQ", "url": "https://x"}}])),
        doc(json!([{"type": "embed", "attrs": {"provider": "youtube", "id": "dQw4w9WgXcQ"}, "content": []}])),
        doc(json!([para(embed("youtube", "dQw4w9WgXcQ"))])),
        // Tables: rows of cells of paragraphs, rectangular, header row first only.
        doc(json!([{"type": "table", "content": []}])),
        doc(json!([{"type": "table"}])),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": []}]}])),
        doc(json!([{"type": "table", "content": [para(text("x"))]}])),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": [para(text("x"))]}]}])),
        doc(
            json!([{"type": "table", "content": [{"type": "tableRow", "content": [{"type": "tableCell", "content": []}]}]}]),
        ),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": [{"type": "tableCell"}]}]}])),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableCell", "content": [{"type": "bulletList", "content": []}]}
        ]}]}])),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableCell", "content": [{"type": "image", "attrs": {"media": MEDIA}}]}
        ]}]}])),
        doc(json!([{"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableCell", "attrs": {"colspan": 2}, "content": [para(text("x"))]}
        ]}]}])),
        doc(
            json!([{"type": "table", "attrs": {"style": "x"}, "content": [{"type": "tableRow", "content": [cell("tableCell", "x")]}]}]),
        ),
        doc(json!([{"type": "table", "content": [
            {"type": "tableRow", "content": [cell("tableCell", "a"), cell("tableCell", "b")]},
            {"type": "tableRow", "content": [cell("tableCell", "c")]}
        ]}])),
        doc(json!([{"type": "table", "content": [
            {"type": "tableRow", "content": [cell("tableHeader", "a"), cell("tableCell", "b")]}
        ]}])),
        doc(json!([{"type": "table", "content": [
            {"type": "tableRow", "content": [cell("tableCell", "a")]},
            {"type": "tableRow", "content": [cell("tableHeader", "b")]}
        ]}])),
        doc(
            json!([{"type": "table", "content": [{"type": "tableRow", "content": [cell("tableCell", "a")]}, table(1, 1)]}]),
        ),
        doc(json!([{"type": "tableRow", "content": [cell("tableCell", "a")]}])),
        doc(json!([cell("tableCell", "a")])),
        // Callouts.
        doc(json!([{"type": "callout", "attrs": {"tone": "danger"}, "content": [para(text("x"))]}])),
        doc(json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [table(1, 1)]}])),
        doc(json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [
            {"type": "callout", "attrs": {"tone": "warn"}, "content": [para(text("x"))]}
        ]}])),
        doc(json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [{"type": "bulletList", "content": [
            {"type": "listItem", "content": [para(text("x")), table(1, 1)]}
        ]}]}])),
        doc(
            json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [{"type": "blockquote", "content": [para(text("x"))]}]}]),
        ),
        doc(json!([{"type": "callout", "content": [para(text("x"))]}])),
        doc(json!([{"type": "callout", "attrs": {"tone": "info"}, "content": []}])),
        doc(json!([{"type": "callout", "attrs": {"tone": "info", "icon": "x"}, "content": [para(text("x"))]}])),
        doc(
            json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [{"type": "heading", "attrs": {"level": 2}, "content": [text("x")]}]}]),
        ),
    ] {
        let err = parse(&bad).expect_err(&bad.to_string());
        // Every message has a code the panel translates.
        assert_ne!(rustcms_core::i18n::Issue::of(&err.0).code, rustcms_core::i18n::CUSTOM, "{err}");
    }
}

#[test]
fn version_2_nodes_parse_and_render() {
    let value = doc(json!([
        {"type": "heading", "attrs": {"level": 2}, "content": [text("Bảng giá")]},
        {"type": "table", "content": [
            {"type": "tableRow", "content": [cell("tableHeader", "Gói"), cell("tableHeader", "Giá <b>")]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [para(json!({"type": "text", "text": "Pro", "marks": [{"type": "bold"}]})), para(text("2"))]},
                cell("tableCell", "10 $")
            ]}
        ]},
        {"type": "table", "content": [{"type": "tableRow", "content": [cell("tableCell", "no header")]}]},
        {"type": "image", "attrs": {"media": MEDIA, "alt": "Cat", "caption": "A <cat>", "width": "wide"}},
        {"type": "image", "attrs": {"media": MEDIA, "alt": "", "caption": "", "width": "normal"}},
        embed("youtube", "dQw4w9WgXcQ"),
        embed("vimeo", "76979871"),
        {"type": "callout", "attrs": {"tone": "warn"}, "content": [para(text("Careful")), {"type": "bulletList", "content": [
            {"type": "listItem", "content": [para(text("one"))]}
        ]}]},
        {"type": "blockquote", "content": [{"type": "heading", "attrs": {"level": 2}, "content": [text("Bảng giá")]}]}
    ]));
    let parsed = parse(&value).unwrap();
    let Node::Table(t) = &parsed.content[1] else { panic!() };
    assert!(t.header && t.rows.len() == 2 && t.rows[1][0].content.len() == 2);
    let Node::Table(t) = &parsed.content[2] else { panic!() };
    assert!(!t.header);
    assert_eq!(
        parsed.content[3],
        Node::Image(Image {
            media: uuid::Uuid::parse_str(MEDIA).unwrap(),
            alt: "Cat".into(),
            caption: "A <cat>".into(),
            width: ImageWidth::Wide
        })
    );
    let images = |info: ImageInfo| std::collections::HashMap::from([(uuid::Uuid::parse_str(MEDIA).unwrap(), info)]);
    let html = to_html_with(&parsed, &images(ImageInfo::new(format!("/m/{MEDIA}")).size(800, 600)));
    assert_eq!(
        html,
        "<h2 id=\"bang-gia\">Bảng giá</h2>\
         <table><thead><tr><th><p>Gói</p></th><th><p>Giá &lt;b&gt;</p></th></tr></thead>\
         <tbody><tr><td><p><strong>Pro</strong></p><p>2</p></td><td><p>10 $</p></td></tr></tbody></table>\
         <table><tbody><tr><td><p>no header</p></td></tr></tbody></table>\
         <figure data-width=\"wide\"><img src=\"/m/0199d3a4-1b2c-7d3e-8f40-123456789abc\" width=\"800\" height=\"600\" alt=\"Cat\">\
         <figcaption>A &lt;cat&gt;</figcaption></figure>\
         <figure><img src=\"/m/0199d3a4-1b2c-7d3e-8f40-123456789abc\" width=\"800\" height=\"600\" alt=\"\"></figure>\
         <figure data-embed=\"youtube\" data-id=\"dQw4w9WgXcQ\"><a href=\"https://www.youtube.com/watch?v=dQw4w9WgXcQ\" rel=\"noopener noreferrer\">https://www.youtube.com/watch?v=dQw4w9WgXcQ</a></figure>\
         <figure data-embed=\"vimeo\" data-id=\"76979871\"><a href=\"https://vimeo.com/76979871\" rel=\"noopener noreferrer\">https://vimeo.com/76979871</a></figure>\
         <aside class=\"callout\" data-tone=\"warn\"><p>Careful</p><ul><li><p>one</p></li></ul></aside>\
         <blockquote><h2 id=\"bang-gia-2\">Bảng giá</h2></blockquote>"
    );
    // A size is written only when both dimensions are known.
    let partial =
        to_html_with(&parsed, &images(ImageInfo { url: format!("/m/{MEDIA}"), width: Some(1), height: None }));
    assert!(!partial.contains("width=\"1\""));
    // `to_html` keeps its signature: figures without sizes.
    assert!(
        to_html(&parsed, &|_| Some("/i".into())).contains("<figure data-width=\"wide\"><img src=\"/i\" alt=\"Cat\">")
    );
    // Images whose URL is unknown are left out with their caption.
    assert!(!to_html_with(&parsed, &Default::default()).contains("figcaption"));
    assert_eq!(media_ids(&parsed).len(), 1);
}

#[test]
fn headings_get_stable_folded_unique_ids() {
    assert_eq!(slug("Tiếng Việt có dấu"), "tieng-viet-co-dau");
    assert_eq!(slug("Đà Nẵng — ĐƯỜNG Phố"), "da-nang-duong-pho");
    assert_eq!(slug("  Hello,   World!! "), "hello-world");
    assert_eq!(slug("Crème brûlée, Smørrebrød & Straße"), "creme-brulee-smorrebrod-strasse");
    assert_eq!(slug("İstanbul"), "istanbul");
    assert_eq!(slug("日本語 2"), "2");
    assert_eq!(slug("Ước mơ ở Huế — Ỷ Ợ"), "uoc-mo-o-hue-y-o");
    assert_eq!(slug("🎉 !!"), "");
    let long = slug(&"ab ".repeat(100));
    assert!(long.chars().count() <= MAX_SLUG && !long.ends_with('-'), "{long}");
    let h = |level: u64, t: &str| json!({"type": "heading", "attrs": {"level": level}, "content": [text(t)]});
    let parsed = parse(&doc(json!([
        h(2, "Giới thiệu"),
        h(3, "Giới thiệu"),
        h(2, "giới-thiệu-2"),
        h(4, "Giới thiệu"),
        h(2, "🎉"),
        h(3, "🎉"),
        {"type": "heading", "attrs": {"level": 2}, "content": [
            {"type": "text", "text": "Bold", "marks": [{"type": "bold"}]}, {"type": "hardBreak"}, text("line")
        ]},
        {"type": "bulletList", "content": [{"type": "listItem", "content": [h(3, "Nested")]}]},
    ])))
    .unwrap();
    let outline = outline(&parsed);
    let ids: Vec<_> = outline.iter().map(|h| (h.level, h.id.as_str())).collect();
    assert_eq!(
        ids,
        [
            (2, "gioi-thieu"),
            (3, "gioi-thieu-2"),
            (2, "gioi-thieu-2-2"),
            (4, "gioi-thieu-3"),
            (2, "section"),
            (3, "section-2"),
            (2, "bold-line"),
            (3, "nested")
        ]
    );
    assert_eq!(outline[0].text, "Giới thiệu");
    assert_eq!(outline[6].text, "Bold line");
    // `to_html` uses the same ids, in the same order.
    let html = to_html(&parsed, &|_| None);
    let in_html: Vec<&str> = html.split("id=\"").skip(1).map(|s| &s[..s.find('"').unwrap()]).collect();
    assert_eq!(in_html, outline.iter().map(|h| h.id.as_str()).collect::<Vec<_>>());
    // Stable: the same document gives the same ids.
    assert_eq!(super::outline(&parsed), outline);
}

#[test]
fn repeated_headings_cost_the_same_each() {
    // Every repeat of a heading starts from its base's last suffix (not from
    // `-2`), so thousands of identical headings render in linear time; a
    // literal `a-2` taken before still pushes `a`'s repeat on.
    let h = |level: u64, t: &str| json!({"type": "heading", "attrs": {"level": level}, "content": [text(t)]});
    let mut content = vec![h(2, "A 2"), h(2, "A")];
    content.extend((0..6_000).map(|_| json!({"type": "heading", "attrs": {"level": 2}})));
    content.push(h(2, "A"));
    let parsed = parse(&doc(Value::Array(content))).unwrap();
    let started = std::time::Instant::now();
    let ids: Vec<String> = outline(&parsed).into_iter().map(|h| h.id).collect();
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(&ids[..3], ["a-2", "a", "section"]);
    assert_eq!(ids[6_001], "section-6000");
    assert_eq!(ids[6_002], "a-3");
}

#[test]
fn documents_are_bounded() {
    let many: Vec<Value> = (0..10_001).map(|_| json!({"type": "horizontalRule"})).collect();
    assert!(parse(&doc(Value::Array(many))).is_err());
    let mut nested = json!({"type": "paragraph", "content": [{"type": "text", "text": "x"}]});
    for _ in 0..20 {
        nested = json!({"type": "blockquote", "content": [nested]});
    }
    assert!(parse(&doc(json!([nested]))).is_err());
    let marks: Vec<Value> = (0..15_000).map(|_| json!({"type": "bold"})).collect();
    assert!(
        parse(&doc(json!([{"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": marks}]}])))
            .is_err()
    );
    let big = "x".repeat(MAX_BYTES);
    assert!(parse(&doc(json!([{"type": "paragraph", "content": [{"type": "text", "text": big}]}]))).is_err());

    // Tables: at most 50 rows of 20 cells.
    assert!(parse(&doc(json!([table(MAX_TABLE_ROWS, MAX_TABLE_COLUMNS)]))).is_ok());
    for (rows, columns, code) in
        [(MAX_TABLE_ROWS + 1, 1, "document_table_rows"), (1, MAX_TABLE_COLUMNS + 1, "document_table_columns")]
    {
        let err = parse(&doc(json!([table(rows, columns)]))).unwrap_err();
        assert_eq!(rustcms_core::i18n::Issue::of(&err.0).code, code);
    }
    // Table rows and cells count as elements: the largest table is 3,051 of them
    // (table, 50 rows, 1,000 cells, their paragraphs and texts).
    let filled = |breaks: usize| {
        let inline: Vec<Value> = (0..breaks).map(|_| json!({"type": "hardBreak"})).collect();
        doc(json!([table(MAX_TABLE_ROWS, MAX_TABLE_COLUMNS), {"type": "paragraph", "content": inline}]))
    };
    let room = MAX_NODES - 3_051 - 1;
    assert!(parse(&filled(room)).is_ok());
    assert_eq!(parse(&filled(room + 1)).unwrap_err().0, format!("may not have more than {MAX_NODES} elements"));
    // Tables nest four levels below their block: the depth limit still holds.
    let mut nested = table(1, 1);
    for _ in 0..13 {
        nested = json!({"type": "blockquote", "content": [nested]});
    }
    assert_eq!(parse(&doc(json!([nested]))).unwrap_err().0, "is nested too deeply");

    // Embeds and images are counted: the bound is accepted, one more refused.
    let embeds: Vec<Value> = (0..MAX_EMBEDS).map(|_| embed("vimeo", "1")).collect();
    assert!(parse(&doc(Value::Array(embeds))).is_ok());
    let embeds: Vec<Value> = (0..=MAX_EMBEDS).map(|_| embed("vimeo", "1")).collect();
    assert_eq!(parse(&doc(Value::Array(embeds))).unwrap_err().0, format!("may not have more than {MAX_EMBEDS} embeds"));
    let images: Vec<Value> = (0..MAX_IMAGES).map(|_| json!({"type": "image", "attrs": {"media": MEDIA}})).collect();
    assert!(parse(&doc(Value::Array(images))).is_ok());
    let images: Vec<Value> = (0..=MAX_IMAGES).map(|_| json!({"type": "image", "attrs": {"media": MEDIA}})).collect();
    assert!(parse(&doc(Value::Array(images))).is_err());
    let caption = json!([{"type": "image", "attrs": {"media": MEDIA, "caption": "ư".repeat(MAX_CAPTION)}}]);
    assert!(parse(&doc(caption)).is_ok(), "the bound counts characters, not bytes");
}

#[test]
fn embed_addresses_parse_to_an_allowed_provider_and_id() {
    let yt = Some(Embed { provider: EmbedProvider::YouTube, id: "dQw4w9WgXcQ".into() });
    for url in [
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "https://youtube.com/watch?t=42&v=dQw4w9WgXcQ",
        "HTTPS://M.YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
        "https://youtu.be/dQw4w9WgXcQ?si=abc",
        "https://www.youtube.com/embed/dQw4w9WgXcQ",
        "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ#x",
        "  https://www.youtube.com/live/dQw4w9WgXcQ  ",
    ] {
        assert_eq!(Embed::from_url(url), yt, "{url}");
    }
    let vimeo = Some(Embed { provider: EmbedProvider::Vimeo, id: "76979871".into() });
    assert_eq!(Embed::from_url("https://vimeo.com/76979871"), vimeo);
    assert_eq!(Embed::from_url("https://vimeo.com/76979871/abcdef"), vimeo);
    assert_eq!(Embed::from_url("https://player.vimeo.com/video/76979871?h=x"), vimeo);
    for url in [
        "javascript:alert(1)//youtube.com/watch?v=dQw4w9WgXcQ",
        "data:text/html,https://youtu.be/dQw4w9WgXcQ",
        "https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ",
        "https://evil.test/youtube.com/watch?v=dQw4w9WgXcQ",
        "https://user:pw@youtube.com/watch?v=dQw4w9WgXcQ",
        "https://youtube.com:8443/watch?v=dQw4w9WgXcQ",
        "https://www.youtube.com/watch?v=dQw4w9WgXc",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ%22",
        "https://www.youtube.com/watch?v=dQw4w9WgX\"Q",
        "https://www.youtube.com/playlist?list=PL123",
        "https://youtu.be/",
        "https://vimeo.com/channels/staffpicks",
        "https://vimeo.com/0123",
        "https://x.com/rustlang/status/1460323737035677698",
        "https://twitter.com/i/web/status/1460323737035677698",
        "ftp://youtu.be/dQw4w9WgXcQ",
        "https://you tu.be/dQw4w9WgXcQ",
        "not a url",
    ] {
        assert_eq!(Embed::from_url(url), None, "{url}");
    }
    // What `from_url` returns, `parse` accepts.
    let embed = Embed::from_url("https://youtu.be/dQw4w9WgXcQ").unwrap();
    let value = doc(json!([{"type": "embed", "attrs": {"provider": embed.provider.as_str(), "id": embed.id}}]));
    assert!(parse(&value).is_ok());
}

// --- Version 3 ---------------------------------------------------------------

fn marked(marks: &[&str]) -> Value {
    json!({"type": "text", "text": "x", "marks": marks.iter().map(|m| json!({"type": m})).collect::<Vec<_>>()})
}

fn kbd(keys: Value) -> Value {
    json!({"type": "app.kbd", "attrs": {"keys": keys}})
}

fn nodes() -> AppNodes {
    AppNodes::new([
        RichTextNode::inline_atom("app.kbd")
            .label("Shortcut")
            .attr("keys", Attr::text().min(1).max(20))
            .text(|attrs| format!("[{}]", attrs.text("keys"))),
        RichTextNode::block_atom("app.product")
            .attr("id", Attr::uuid())
            .attr("layout", Attr::one_of(["card", "row"]).default("card"))
            .attr("qty", Attr::integer().range(1..=9).default(1))
            .attr("featured", Attr::boolean().default(false)),
        RichTextNode::inline_atom("app.blank").attr("n", Attr::integer().range(1..=99)),
    ])
}

const PRODUCT: &str = "0199D3A4-1B2C-7D3E-8F40-123456789ABC";

#[test]
fn version_3_marks_parse_render_and_exclude_each_other() {
    let value = v3(json!([para(
        marked(&["bold", "italic", "underline", "strike", "link", "superscript"])
            .as_object()
            .map(|o| {
                let mut o = o.clone();
                o["marks"][4] = json!({"type": "link", "attrs": {"href": "https://x.test"}});
                Value::Object(o)
            })
            .unwrap()
    )]));
    let parsed = parse(&value).unwrap();
    assert_eq!(parsed.version, 3);
    assert_eq!(
        to_html(&parsed, &|_| None),
        r#"<p><sup><a href="https://x.test" rel="noopener noreferrer nofollow"><s><u><em><strong>x</strong></em></u></s></a></sup></p>"#
    );
    let sub = parse(&v3(json!([para(marked(&["subscript"]))]))).unwrap();
    assert_eq!(to_html(&sub, &|_| None), "<p><sub>x</sub></p>");
    for (bad, code) in [
        (v3(json!([para(marked(&["superscript", "subscript"]))])), "document_structure"),
        (v3(json!([para(marked(&["underline", "underline"]))])), "document_structure"),
        (doc(json!([para(marked(&["underline"]))])), "document_needs_version"),
        (v1(json!([para(marked(&["superscript"]))])), "document_needs_version"),
        (doc(json!([para(marked(&["subscript"]))])), "document_needs_version"),
        (
            v3(json!([para(
                json!({"type": "text", "text": "x", "marks": [{"type": "underline", "attrs": {"style": "x"}}]})
            )])),
            "document_structure",
        ),
    ] {
        let err = parse(&bad).expect_err(&bad.to_string());
        assert_eq!(rustcms_core::i18n::Issue::of(&err.0).code, code, "{err}");
    }
}

#[test]
fn application_nodes_are_checked_against_their_declaration() {
    let nodes = nodes();
    let value = v3(json!([
        {"type": "heading", "attrs": {"level": 2}, "content": [text("Press "), kbd(json!("Ctrl+C"))]},
        para(kbd(json!("<b>&\"'"))),
        {"type": "app.product", "attrs": {"id": PRODUCT}},
        {"type": "bulletList", "content": [{"type": "listItem", "content": [{"type": "app.product", "attrs": {"id": PRODUCT, "layout": "row"}}]}]},
        {"type": "blockquote", "content": [{"type": "app.product", "attrs": {"id": PRODUCT, "qty": 9, "featured": true}}]},
        {"type": "table", "content": [{"type": "tableRow", "content": [{"type": "tableCell", "content": [para(kbd(json!("A")))]}]}]},
        {"type": "callout", "attrs": {"tone": "info"}, "content": [para(json!({"type": "app.blank", "attrs": {"n": 3}}))]},
    ]));
    let parsed = nodes.parse(&value).unwrap();
    // Defaults filled in, UUIDs in lower case.
    let Node::App(product) = &parsed.content[2] else { panic!() };
    assert_eq!(
        Value::Object(product.attrs.as_map().clone()),
        json!({"id": PRODUCT.to_lowercase(), "layout": "card", "qty": 1, "featured": false})
    );
    let completed = nodes.complete(&value).unwrap();
    assert_eq!(completed["content"][2]["attrs"], Value::Object(product.attrs.as_map().clone()));
    assert_eq!(completed["content"][0], value["content"][0], "the rest is left as it is");
    // The plain parser and other lists know none of them.
    let err = parse(&value).unwrap_err();
    assert_eq!(err.0, "has an unknown element `app.kbd`");
    let err = nodes.parse_for(&value, &["app.kbd", "app.blank"]).unwrap_err();
    assert_eq!(err.0, "has an unknown element `app.product`");
    assert!(nodes.parse_for(&value, &["app.kbd", "app.blank", "app.product"]).is_ok());

    // HTML: escaped attributes and texts; outline with the node texts.
    let html = Html::new(&parsed).nodes(&nodes).render();
    assert!(
        html.starts_with(
            r#"<h2 id="press-ctrl-c">Press <span data-node="app.kbd" data-keys="Ctrl+C">[Ctrl+C]</span></h2>"#
        ),
        "{html}"
    );
    assert!(html.contains(r#"<p><span data-node="app.kbd" data-keys="&lt;b&gt;&amp;&quot;&#39;">[&lt;b&gt;&amp;&quot;&#39;]</span></p>"#), "{html}");
    assert!(
        html.contains(&format!(
            r#"<div data-node="app.product" data-featured="false" data-id="{}" data-layout="card" data-qty="1"></div>"#,
            PRODUCT.to_lowercase()
        )),
        "{html}"
    );
    assert_eq!(Html::new(&parsed).nodes(&nodes).outline()[0].text, "Press [Ctrl+C]");
    // Without the declarations: the elements, no text.
    let bare = to_html(&parsed, &|_| None);
    assert!(
        bare.starts_with(r#"<h2 id="press">Press <span data-node="app.kbd" data-keys="Ctrl+C"></span></h2>"#),
        "{bare}"
    );
    assert_eq!(outline(&parsed)[0].id, "press");

    for (bad, message) in [
        (v3(json!([para(kbd(json!("")))])), "has an element `app.kbd` with an invalid attribute `keys`"),
        (v3(json!([para(kbd(json!("x".repeat(21))))])), "has an element `app.kbd` with an invalid attribute `keys`"),
        (v3(json!([para(kbd(json!(1)))])), "has an element `app.kbd` with an invalid attribute `keys`"),
        (v3(json!([para(json!({"type": "app.kbd"}))])), "has an element `app.kbd` without its attribute `keys`"),
        (
            v3(json!([para(json!({"type": "app.kbd", "attrs": {"keys": "x", "html": "<b>"}}))])),
            "has an element `app.kbd` with an invalid attribute `html`",
        ),
        (
            v3(json!([para(json!({"type": "app.kbd", "attrs": {"keys": "x"}, "content": []}))])),
            "has unexpected properties on `app.kbd`",
        ),
        (
            v3(json!([para(json!({"type": "app.kbd", "attrs": {"keys": "x"}, "text": "x"}))])),
            "has unexpected properties on `app.kbd`",
        ),
        (
            v3(json!([para(json!({"type": "app.kbd", "attrs": {"keys": "x"}, "marks": [{"type": "bold"}]}))])),
            "has unexpected properties on `app.kbd`",
        ),
        (v3(json!([kbd(json!("x"))])), "has an element `app.kbd` out of place"),
        (
            v3(json!([para(json!({"type": "app.product", "attrs": {"id": PRODUCT}}))])),
            "has an element `app.product` out of place",
        ),
        (
            v3(json!([{"type": "app.product", "attrs": {"id": "nope"}}])),
            "has an element `app.product` with an invalid attribute `id`",
        ),
        (
            v3(json!([{"type": "app.product", "attrs": {"id": PRODUCT, "layout": "grid"}}])),
            "has an element `app.product` with an invalid attribute `layout`",
        ),
        (
            v3(json!([{"type": "app.product", "attrs": {"id": PRODUCT, "qty": 10}}])),
            "has an element `app.product` with an invalid attribute `qty`",
        ),
        (
            v3(json!([{"type": "app.product", "attrs": {"id": PRODUCT, "qty": 1.5}}])),
            "has an element `app.product` with an invalid attribute `qty`",
        ),
        (
            v3(json!([{"type": "app.product", "attrs": {"id": PRODUCT, "featured": "yes"}}])),
            "has an element `app.product` with an invalid attribute `featured`",
        ),
        (
            v3(
                json!([{"type": "callout", "attrs": {"tone": "info"}, "content": [{"type": "app.product", "attrs": {"id": PRODUCT}}]}]),
            ),
            "has a callout holding something else than paragraphs and lists",
        ),
        (
            v3(
                json!([{"type": "table", "content": [{"type": "tableRow", "content": [{"type": "tableCell", "content": [{"type": "app.product", "attrs": {"id": PRODUCT}}]}]}]}]),
            ),
            "has a table cell holding something else than paragraphs",
        ),
        (
            v3(json!([{"type": "codeBlock", "content": [kbd(json!("x"))]}])),
            "has a code block holding something else than plain text",
        ),
        (v3(json!([para(json!({"type": "app.nope", "attrs": {}}))])), "has an unknown element `app.nope`"),
        (doc(json!([para(kbd(json!("x")))])), "has the element `app.kbd`, which needs document version 3"),
    ] {
        let err = nodes.parse(&bad).expect_err(&bad.to_string());
        assert_eq!(err.0, message, "{bad}");
        assert_ne!(rustcms_core::i18n::Issue::of(&err.0).code, rustcms_core::i18n::CUSTOM, "{err}");
    }
}

#[test]
fn application_nodes_are_bounded() {
    let nodes = nodes();
    let blank = json!({"type": "app.blank", "attrs": {"n": 1}});
    let fits = v3(json!([{"type": "paragraph", "content": vec![blank.clone(); MAX_APP_NODES]}]));
    assert!(nodes.parse(&fits).is_ok());
    let over = v3(json!([{"type": "paragraph", "content": vec![blank; MAX_APP_NODES + 1]}]));
    let err = nodes.parse(&over).unwrap_err();
    assert_eq!(err.0, format!("may not have more than {MAX_APP_NODES} application elements"));
    assert_eq!(rustcms_core::i18n::Issue::of(&err.0).code, "max_app_nodes");
}

#[test]
fn declarations_are_checked() {
    let panics = |f: fn()| std::panic::catch_unwind(f).expect_err("panics");
    for bad in [
        (|| drop(AppNodes::new([RichTextNode::inline_atom("kbd")]))) as fn(),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.Kbd")])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.")])),
        || drop(AppNodes::new([RichTextNode::inline_atom(format!("app.{}", "a".repeat(33)))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a"), RichTextNode::block_atom("app.a")])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").label(" ")])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::text()).attr("x", Attr::text())])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("data-x", Attr::text())])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::text().max(MAX_TEXT_ATTR + 1))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::text().min(5).max(2))])),
        || {
            drop(AppNodes::new([
                RichTextNode::inline_atom("app.a").attr("x", Attr::integer().range(0..=MAX_SAFE_INTEGER + 1))
            ]))
        },
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::one_of(Vec::<String>::new()))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::one_of(["a", "a"]))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::one_of(["a"]).default("b"))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::integer().range(1..=2).default(3))])),
        || drop(AppNodes::new([RichTextNode::inline_atom("app.a").attr("x", Attr::uuid().default(PRODUCT))])),
        || drop(Attr::boolean().max(3)),
        || {
            let mut node = RichTextNode::inline_atom("app.a");
            for i in 0..=MAX_NODE_ATTRS {
                node = node.attr(format!("a{i}"), Attr::boolean());
            }
            drop(AppNodes::new([node]));
        },
    ] {
        panics(bad);
    }
}
