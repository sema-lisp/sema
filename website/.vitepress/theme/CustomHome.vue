<template>
<div class="custom-home">

<!-- ══════════════════════════════════════════════════════════
     HERO
     ══════════════════════════════════════════════════════════ -->
<section class="hero-split">
  <div class="hero-text">
    <h1 class="hero-headline">
      Prompts are s&#8209;expressions.<br>
      Conversations are data.<br>
      <span class="hero-accent">LLMs are just evaluation.</span>
    </h1>
    <p class="hero-sub">
      A Scheme-like Lisp where completions, tool use, and agentic loops are
      native forms &mdash; not string templates bolted onto a scripting language.
      Implemented in Rust. 450+ builtins. 11 providers. Bytecode VM.
    </p>
    <div class="hero-actions">
      <a href="/docs/" class="hero-btn primary">Get Started</a>
      <a href="https://github.com/sema-lisp/sema" class="hero-btn secondary">GitHub</a>
    </div>
    <div class="hero-install-inline">
      <span class="prompt">$</span> brew install helgesverre/tap/sema-lang
    </div>
  </div>
  <div class="hero-code">
<pre><code><span class="c">;; Define a tool the LLM can call</span>
<span class="p">(</span><span class="k">deftool</span> get-weather
  <span class="s">"Get weather for a city"</span>
  <span class="p">{</span><span class="kw">:city</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>city<span class="p">)</span>
    <span class="p">(</span><span class="b">format</span> <span class="s">"~a: 22°C, sunny"</span> city<span class="p">)))</span>

<span class="c">;; Build an agent with tools</span>
<span class="p">(</span><span class="k">defagent</span> weather-bot
  <span class="p">{</span><span class="kw">:system</span> <span class="s">"You answer weather questions."</span>
   <span class="kw">:tools</span>  <span class="p">[</span>get-weather<span class="p">]</span>
   <span class="kw">:model</span>  <span class="s">"claude-haiku-4-5-20251001"</span><span class="p">})</span>

<span class="p">(</span><span class="b">agent/run</span> weather-bot
  <span class="s">"What's the weather in Tokyo?"</span><span class="p">)</span>
<span class="c">; =&gt; "The weather in Tokyo is 22°C and sunny."</span></code></pre>
  </div>
</section>

<!-- ═══════════════════════════════════════════════════════
     LLM PRIMITIVES
     ═══════════════════════════════════════════════════════ -->

<section id="llm" class="split">
  <div class="split-text">
    <div class="label">LLM Primitives</div>
    <h2><span class="paren">(</span>Prompts are data<span class="paren">)</span></h2>
    <p>
      Conversations are persistent values. Prompts compose like any other
      s-expression. Completions, chat, structured extraction, classification,
      tool use, and agentic loops&mdash;all as native forms.
    </p>
    <p>
      11 providers auto-configured from environment variables.
      Response caching, cost budgets, rate limiting, fallback chains,
      and batch processing built in.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Simple completion</span>
<span class="p">(</span><span class="b">llm/complete</span> <span class="s">"Say hello in 5 words"</span>
  <span class="p">{</span><span class="kw">:max-tokens</span> <span class="n">50</span><span class="p">})</span>

<span class="c">;; Chat with roles</span>
<span class="p">(</span><span class="b">llm/chat</span>
  <span class="p">(</span><span class="b">list</span> <span class="p">(</span><span class="b">message</span> <span class="kw">:system</span> <span class="s">"You are helpful."</span><span class="p">)</span>
        <span class="p">(</span><span class="b">message</span> <span class="kw">:user</span> <span class="s">"What is Lisp?"</span><span class="p">))</span>
  <span class="p">{</span><span class="kw">:max-tokens</span> <span class="n">100</span><span class="p">})</span>

<span class="c">;; Structured extraction</span>
<span class="p">(</span><span class="b">llm/extract</span>
  <span class="p">{</span><span class="kw">:vendor</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}</span>
   <span class="kw">:amount</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:number</span><span class="p">}}</span>
  <span class="s">"Coffee $4.50 at Blue Bottle"</span><span class="p">)</span>
<span class="c">; =&gt; {:amount 4.5 :vendor "Blue Bottle"}</span>

<span class="c">;; Classification</span>
<span class="p">(</span><span class="b">llm/classify</span> <span class="p">(</span><span class="b">list</span> <span class="kw">:positive</span> <span class="kw">:negative</span> <span class="kw">:neutral</span><span class="p">)</span>
  <span class="s">"This product is amazing!"</span><span class="p">)</span>
<span class="c">; =&gt; :positive</span></code></pre>
  </div>
</section>

<!-- ——— Tools & Agents ——— -->
<section class="split reverse compact">
  <div class="split-text">
    <div class="label">Function Calling</div>
    <h2><span class="paren">(</span>Tools &amp; Agents<span class="paren">)</span></h2>
    <p>
      Define tools with <code class="ic">deftool</code>&mdash;the LLM sees the
      schema, calls your Lisp function, and uses the result. Parameters are
      converted from JSON to Sema values automatically.
    </p>
    <p>
      <code class="ic">defagent</code> combines a system prompt, tools, and a
      multi-turn loop. The agent calls tools and reasons until it has an
      answer or hits <code class="ic">:max-turns</code>.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Define a tool</span>
<span class="p">(</span><span class="k">deftool</span> lookup-capital
  <span class="s">"Look up the capital of a country"</span>
  <span class="p">{</span><span class="kw">:country</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span>
             <span class="kw">:description</span> <span class="s">"Country name"</span><span class="p">}}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>country<span class="p">)</span>
    <span class="p">(</span><span class="k">cond</span>
      <span class="p">((</span><span class="b">=</span> country <span class="s">"Norway"</span><span class="p">)</span> <span class="s">"Oslo"</span><span class="p">)</span>
      <span class="p">((</span><span class="b">=</span> country <span class="s">"France"</span><span class="p">)</span> <span class="s">"Paris"</span><span class="p">)</span>
      <span class="p">(</span>else <span class="s">"Unknown"</span><span class="p">))))</span>

<span class="c">;; Use tools in chat</span>
<span class="p">(</span><span class="b">llm/chat</span>
  <span class="p">(</span><span class="b">list</span> <span class="p">(</span><span class="b">message</span> <span class="kw">:user</span> <span class="s">"Capital of Norway?"</span><span class="p">))</span>
  <span class="p">{</span><span class="kw">:tools</span> <span class="p">(</span><span class="b">list</span> lookup-capital<span class="p">)})</span>

<span class="c">;; Agent with multi-turn loop</span>
<span class="p">(</span><span class="k">defagent</span> geography-bot
  <span class="p">{</span><span class="kw">:system</span> <span class="s">"You answer geography questions."</span>
   <span class="kw">:tools</span>  <span class="p">(</span><span class="b">list</span> lookup-capital<span class="p">)</span>
   <span class="kw">:max-turns</span> <span class="n">3</span><span class="p">})</span>
<span class="p">(</span><span class="b">agent/run</span> geography-bot <span class="s">"Capital of France?"</span><span class="p">)</span></code></pre>
  </div>
</section>

<!-- ——— Real-World Showcase ——— -->
<section class="split compact">
  <div class="split-text">
    <div class="label">In Practice</div>
    <h2><span class="paren">(</span>Build real tools<span class="paren">)</span></h2>
    <p>
      A full coding agent in 25 lines. Tools are just lambdas with a schema.
      The agent loop handles tool dispatch, retries, and conversation
      management automatically.
    </p>
    <p>
      Or extract data from PDFs, build semantic search, analyze images&mdash;all
      with builtins. No external databases or SDKs required.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; A coding agent in 25 lines</span>

<span class="p">(</span><span class="k">deftool</span> read-file
  <span class="s">"Read a file's contents"</span>
  <span class="p">{</span><span class="kw">:path</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>path<span class="p">)</span> <span class="p">(</span><span class="b">file/read</span> path<span class="p">)))</span>

<span class="p">(</span><span class="k">deftool</span> run-command
  <span class="s">"Run a shell command"</span>
  <span class="p">{</span><span class="kw">:command</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>command<span class="p">)</span>
    <span class="p">(</span><span class="k">define</span> r <span class="p">(</span><span class="b">shell</span> <span class="s">"sh"</span> <span class="s">"-c"</span> command<span class="p">))</span>
    <span class="p">(</span><span class="b">string-append</span> <span class="p">(</span><span class="kw">:stdout</span> r<span class="p">)</span> <span class="p">(</span><span class="kw">:stderr</span> r<span class="p">))))</span>

<span class="p">(</span><span class="k">defagent</span> coder
  <span class="p">{</span><span class="kw">:system</span> <span class="s">"You are a coding assistant.
            Read files before editing.
            Run tests after changes."</span>
   <span class="kw">:tools</span>  <span class="p">[</span>read-file run-command<span class="p">]</span>
   <span class="kw">:max-turns</span> <span class="n">10</span><span class="p">})</span>

<span class="p">(</span><span class="b">agent/run</span> coder
  <span class="s">"Find all TODO comments in src/"</span><span class="p">)</span></code></pre>
  </div>
</section>

<!-- ——— Document Q&A (RAG) ——— -->
<section class="split reverse compact">
  <div class="split-text">
    <div class="label">Retrieval</div>
    <h2><span class="paren">(</span>Document Q&amp;A<span class="paren">)</span></h2>
    <p>
      Ingest a PDF, embed each page into a vector store, and answer questions
      with retrieval&mdash;no external database, no LangChain, no boilerplate.
    </p>
    <p>
      Built-in PDF extraction with <code class="ic">pdf/extract-text-pages</code>,
      embeddings via <code class="ic">llm/embed</code>, and an in-memory
      vector store with cosine similarity search and disk persistence.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; PDF → vector store → answer</span>
<span class="p">(</span><span class="b">vector-store/create</span> <span class="s">"manual"</span><span class="p">)</span>

<span class="p">(</span><span class="k">define</span> page-num <span class="n">0</span><span class="p">)</span>
<span class="p">(</span><span class="b">for-each</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>page<span class="p">)</span>
    <span class="p">(</span><span class="k">set!</span> page-num <span class="p">(</span><span class="b">+</span> page-num <span class="n">1</span><span class="p">))</span>
    <span class="p">(</span><span class="b">vector-store/add</span> <span class="s">"manual"</span>
      <span class="p">(</span><span class="b">format</span> <span class="s">"p~a"</span> page-num<span class="p">)</span>
      <span class="p">(</span><span class="b">llm/embed</span> page<span class="p">)</span> <span class="p">{</span><span class="kw">:text</span> page<span class="p">}))</span>
  <span class="p">(</span><span class="b">pdf/extract-text-pages</span> <span class="s">"manual.pdf"</span><span class="p">))</span>

<span class="c">;; Ask a question</span>
<span class="p">(</span><span class="k">define</span> hits
  <span class="p">(</span><span class="b">vector-store/search</span> <span class="s">"manual"</span>
    <span class="p">(</span><span class="b">llm/embed</span> <span class="s">"How do I configure providers?"</span><span class="p">)</span> <span class="n">3</span><span class="p">))</span>

<span class="p">(</span><span class="b">llm/complete</span>
  <span class="p">(</span><span class="k">prompt</span>
    <span class="p">(</span>system <span class="s">"Answer using only this context."</span><span class="p">)</span>
    <span class="p">(</span>user <span class="p">(</span><span class="b">string/join</span>
      <span class="p">(</span><span class="b">map</span> <span class="p">(</span><span class="k">fn</span> <span class="p">(</span>h<span class="p">)</span> <span class="p">(</span><span class="kw">:text</span> <span class="p">(</span><span class="kw">:metadata</span> h<span class="p">)))</span> hits<span class="p">)</span>
      <span class="s">"\n---\n"</span><span class="p">))</span>
    <span class="p">(</span>user <span class="s">"How do I configure providers?"</span><span class="p">)))</span></code></pre>
  </div>
</section>

<!-- ——— Vision Extraction ——— -->
<section class="split compact">
  <div class="split-text">
    <div class="label">Multimodal</div>
    <h2><span class="paren">(</span>Vision<span class="paren">)</span></h2>
    <p>
      Extract structured data from images with
      <code class="ic">llm/extract-from-image</code>&mdash;same schema syntax as
      <code class="ic">llm/extract</code>, but for receipts, screenshots, forms,
      or any visual input.
    </p>
    <p>
      Works with OpenAI, Anthropic, and Ollama vision models.
      Images can also be attached to conversations with
      <code class="ic">message/with-image</code>.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Extract typed data from an image</span>
<span class="p">(</span><span class="b">llm/extract-from-image</span>
  <span class="p">{</span><span class="kw">:vendor</span> <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}</span>
   <span class="kw">:total</span>  <span class="p">{</span><span class="kw">:type</span> <span class="kw">:number</span><span class="p">}</span>
   <span class="kw">:date</span>   <span class="p">{</span><span class="kw">:type</span> <span class="kw">:string</span><span class="p">}}</span>
  <span class="s">"receipt.jpg"</span><span class="p">)</span>
<span class="c">; =&gt; {:vendor "Blue Bottle" :total 4.5 :date "2025-02-18"}</span>

<span class="c">;; Vision in conversations</span>
<span class="p">(</span><span class="k">define</span> img <span class="p">(</span><span class="b">file/read-bytes</span> <span class="s">"screenshot.png"</span><span class="p">))</span>
<span class="p">(</span><span class="b">llm/chat</span>
  <span class="p">(</span><span class="b">list</span> <span class="p">(</span><span class="b">message/with-image</span> <span class="kw">:user</span>
     <span class="s">"What error is shown?"</span> img<span class="p">))</span>
  <span class="p">{</span><span class="kw">:max-tokens</span> <span class="n">200</span><span class="p">})</span></code></pre>
  </div>
</section>

<!-- ——— Conversations & Streaming ——— -->
<section class="split compact">
  <div class="split-text">
    <div class="label">Stateful Dialogue</div>
    <h2><span class="paren">(</span>Conversations<span class="paren">)</span></h2>
    <p>
      Immutable conversation values that accumulate message history.
      <code class="ic">conversation/say</code> sends a message, gets a reply,
      and returns a new conversation with both appended.
    </p>
    <p>
      Process collections in parallel with <code class="ic">llm/pmap</code>
      and <code class="ic">llm/batch</code>.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Persistent conversations</span>
<span class="p">(</span><span class="k">define</span> c <span class="p">(</span><span class="b">conversation/new</span> <span class="p">{}))</span>
<span class="p">(</span><span class="k">define</span> c <span class="p">(</span><span class="b">conversation/say</span> c
  <span class="s">"Remember: the secret is 7"</span><span class="p">))</span>
<span class="p">(</span><span class="k">define</span> c <span class="p">(</span><span class="b">conversation/say</span> c
  <span class="s">"What is the secret?"</span><span class="p">))</span>
<span class="p">(</span><span class="b">conversation/last-reply</span> c<span class="p">)</span>
<span class="c">; =&gt; "The secret is 7."</span>

<span class="c">;; Parallel batch processing</span>
<span class="p">(</span><span class="b">llm/pmap</span>
  <span class="p">(</span><span class="k">fn</span> <span class="p">(</span>word<span class="p">)</span> <span class="p">(</span><span class="b">format</span> <span class="s">"Define: ~a"</span> word<span class="p">))</span>
  '<span class="p">(</span><span class="s">"serendipity"</span> <span class="s">"ephemeral"</span><span class="p">)</span>
  <span class="p">{</span><span class="kw">:max-tokens</span> <span class="n">50</span><span class="p">})</span>

<span class="c">;; Budget-scoped calls</span>
<span class="p">(</span><span class="b">llm/with-budget</span> <span class="p">{</span><span class="kw">:max-cost-usd</span> <span class="n">1.00</span><span class="p">}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">()</span>
    <span class="p">(</span><span class="b">llm/complete</span> <span class="s">"Summarize this"</span><span class="p">)))</span>

<span class="c">;; Cache repeated calls</span>
<span class="p">(</span><span class="b">llm/with-cache</span> <span class="p">{</span><span class="kw">:ttl</span> <span class="n">3600</span><span class="p">}</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">()</span>
    <span class="p">(</span><span class="b">llm/complete</span> <span class="s">"Same prompt"</span><span class="p">)))</span></code></pre>
  </div>
</section>

<!-- ——— Vector Store ——— -->
<section class="split reverse compact">
  <div class="split-text">
    <div class="label">Semantic Search</div>
    <h2><span class="paren">(</span>Vector Store<span class="paren">)</span></h2>
    <p>
      Generate embeddings with <code class="ic">llm/embed</code> and compute
      cosine similarity with <code class="ic">llm/similarity</code>.
      Supports Jina, Voyage, Cohere, and OpenAI embedding models.
    </p>
    <p>
      Built-in vector store with <code class="ic">vector-store/create</code>,
      <code class="ic">vector-store/add</code>, and
      <code class="ic">vector-store/search</code> for similarity search.
      Save to disk with <code class="ic">vector-store/save</code> and reload
      with <code class="ic">vector-store/open</code>.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Create a store and add documents</span>
<span class="p">(</span><span class="b">vector-store/create</span> <span class="s">"knowledge"</span><span class="p">)</span>

<span class="p">(</span><span class="b">for-each</span>
  <span class="p">(</span><span class="k">fn</span> <span class="p">(</span>doc<span class="p">)</span>
    <span class="p">(</span><span class="b">vector-store/add</span> <span class="s">"knowledge"</span>
      <span class="p">(</span><span class="b">car</span> doc<span class="p">)</span> <span class="p">(</span><span class="b">llm/embed</span> <span class="p">(</span><span class="b">cadr</span> doc<span class="p">))</span>
      <span class="p">{</span><span class="kw">:text</span> <span class="p">(</span><span class="b">cadr</span> doc<span class="p">)}))</span>
  '<span class="p">((</span><span class="s">"lisp"</span>    <span class="s">"Lisp is a family of programming languages"</span><span class="p">)</span>
    <span class="p">(</span><span class="s">"rust"</span>    <span class="s">"Rust is a systems language focused on safety"</span><span class="p">)</span>
    <span class="p">(</span><span class="s">"cooking"</span> <span class="s">"Italian cooking uses fresh ingredients"</span><span class="p">)))</span>

<span class="c">;; Similarity search</span>
<span class="p">(</span><span class="b">vector-store/search</span> <span class="s">"knowledge"</span>
  <span class="p">(</span><span class="b">llm/embed</span> <span class="s">"writing code"</span><span class="p">)</span> <span class="n">2</span><span class="p">)</span>
<span class="c">; =&gt; ({:id "rust" :score 0.82 :metadata {...}}</span>
<span class="c">;     {:id "lisp" :score 0.78 :metadata {...}})</span>

<span class="c">;; Persist to disk and reload</span>
<span class="p">(</span><span class="b">vector-store/save</span> <span class="s">"knowledge"</span> <span class="s">"knowledge.json"</span><span class="p">)</span>
<span class="p">(</span><span class="b">vector-store/open</span> <span class="s">"reloaded"</span> <span class="s">"knowledge.json"</span><span class="p">)</span></code></pre>
  </div>
</section>

<!-- ——— LLM reference CTA ——— -->
<div class="ref-section" style="text-align: center;">
  <h3>LLM Function Reference</h3>
  <p style="font-size: 1.15rem; color: var(--text); margin-bottom: 1.5rem; font-style: italic;">
    50+ LLM builtins — completion, chat, streaming, tools, agents, embeddings, vector stores, caching, and more.
  </p>
  <a href="/docs/llm/" class="docs-link">Browse LLM Reference →</a>
</div>

<!-- ——— Providers ——— -->
<div class="providers-strip">
  <h3>Supported Providers</h3>
  <div class="provider-list">
    <span>Anthropic</span>
    <span>OpenAI</span>
    <span>Google Gemini</span>
    <span>Ollama</span>
    <span>Groq</span>
    <span>xAI</span>
    <span>Mistral</span>
    <span>Moonshot</span>
    <span>Jina</span>
    <span>Voyage</span>
    <span>Cohere</span>
  </div>
</div>

<!-- ═══════════════════════════════════════════════════════
     THE LANGUAGE
     ═══════════════════════════════════════════════════════ -->

<section id="language" class="split">
  <div class="split-text">
    <div class="label">The Language</div>
    <h2><span class="paren">(</span>Scheme meets Clojure<span class="paren">)</span></h2>
    <p>
      A Scheme-like core with Clojure-style keywords (<code class="ic">:foo</code>),
      map literals (<code class="ic">{:key val}</code>),
      and vector literals (<code class="ic">[1 2 3]</code>).
    </p>
    <p>
      Tail-call optimized via trampoline. Closures, macros,
      higher-order functions, and a module system&mdash;all in a single-threaded
      evaluator small enough to read in an afternoon.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Recursion</span>
<span class="p">(</span><span class="k">define</span> <span class="p">(</span>factorial n<span class="p">)</span>
  <span class="p">(</span><span class="k">if</span> <span class="p">(</span><span class="b">&lt;=</span> n <span class="n">1</span><span class="p">)</span> <span class="n">1</span> <span class="p">(</span><span class="b">*</span> n <span class="p">(</span>factorial <span class="p">(</span><span class="b">-</span> n <span class="n">1</span><span class="p">)))))</span>
<span class="p">(</span>factorial <span class="n">10</span><span class="p">)</span> <span class="c">; =&gt; 3628800</span>

<span class="c">;; Higher-order functions</span>
<span class="p">(</span><span class="b">map</span> <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>x<span class="p">)</span> <span class="p">(</span><span class="b">*</span> x x<span class="p">))</span> <span class="p">(</span><span class="b">range</span> <span class="n">1</span> <span class="n">6</span><span class="p">))</span>
<span class="c">; =&gt; (1 4 9 16 25)</span>

<span class="p">(</span><span class="b">filter</span> <span class="b">even?</span> <span class="p">(</span><span class="b">range</span> <span class="n">1</span> <span class="n">11</span><span class="p">))</span>
<span class="c">; =&gt; (2 4 6 8 10)</span>

<span class="p">(</span><span class="b">foldl</span> <span class="b">+</span> <span class="n">0</span> <span class="p">(</span><span class="b">range</span> <span class="n">1</span> <span class="n">11</span><span class="p">))</span>
<span class="c">; =&gt; 55</span>

<span class="c">;; Maps &mdash; keywords are functions</span>
<span class="p">(</span><span class="k">define</span> person <span class="p">{</span><span class="kw">:name</span> <span class="s">"Ada"</span> <span class="kw">:age</span> <span class="n">36</span><span class="p">})</span>
<span class="p">(</span><span class="kw">:name</span> person<span class="p">)</span>  <span class="c">; =&gt; "Ada"</span>

<span class="c">;; Closures and composition</span>
<span class="p">(</span><span class="k">define</span> <span class="p">(</span>compose f g<span class="p">)</span>
  <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>x<span class="p">)</span> <span class="p">(</span>f <span class="p">(</span>g x<span class="p">))))</span>

<span class="p">(</span><span class="k">define</span> inc-then-double
  <span class="p">(</span>compose <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>x<span class="p">)</span> <span class="p">(</span><span class="b">*</span> x <span class="n">2</span><span class="p">))</span>
           <span class="p">(</span><span class="k">lambda</span> <span class="p">(</span>x<span class="p">)</span> <span class="p">(</span><span class="b">+</span> x <span class="n">1</span><span class="p">))))</span>
<span class="p">(</span>inc-then-double <span class="n">5</span><span class="p">)</span> <span class="c">; =&gt; 12</span></code></pre>
  </div>
</section>

<!-- ——— Resilience & Metaprogramming ——— -->
<section class="split reverse compact">
  <div class="split-text">
    <div class="label">Resilience &amp; Metaprogramming</div>
    <h2><span class="paren">(</span>try, catch, defmacro<span class="paren">)</span></h2>
    <p>
      Structured error handling with typed error maps.
      <code class="ic">catch</code> binds an error map with
      <code class="ic">:type</code>, <code class="ic">:message</code>, and
      <code class="ic">:value</code> keys for user-thrown values.
    </p>
    <p>
      <code class="ic">defmacro</code> with quasiquote, unquote, and splicing.
      <code class="ic">eval</code> and <code class="ic">read</code> for runtime
      code generation. Inspect expansions with <code class="ic">macroexpand</code>.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Error handling</span>
<span class="p">(</span><span class="k">try</span>
  <span class="p">(</span><span class="b">/</span> <span class="n">1</span> <span class="n">0</span><span class="p">)</span>
  <span class="p">(</span><span class="k">catch</span> e
    <span class="p">(</span><span class="b">println</span> <span class="p">(</span><span class="kw">:message</span> e<span class="p">))</span>
    <span class="p">(</span><span class="kw">:type</span> e<span class="p">)))</span>  <span class="c">; =&gt; :eval</span>

<span class="p">(</span><span class="k">throw</span> <span class="p">{</span><span class="kw">:code</span> <span class="n">404</span> <span class="kw">:reason</span> <span class="s">"not found"</span><span class="p">})</span>

<span class="c">;; Macros</span>
<span class="p">(</span><span class="k">defmacro</span> unless <span class="p">(</span>test . body<span class="p">)</span>
  <span class="p">`(</span><span class="k">if</span> ,test <span class="b">nil</span> <span class="p">(</span><span class="k">begin</span> ,@body<span class="p">)))</span>

<span class="p">(</span>unless <span class="b">#f</span>
  <span class="p">(</span><span class="b">println</span> <span class="s">"this runs!"</span><span class="p">))</span>

<span class="c">;; Runtime eval</span>
<span class="p">(</span><span class="k">eval</span> <span class="p">(</span><span class="b">read</span> <span class="s">"(+ 1 2 3)"</span><span class="p">))</span>  <span class="c">; =&gt; 6</span></code></pre>
  </div>
</section>

<!-- ═══════════════════════════════════════════════════════
     STANDARD LIBRARY (CONDENSED)
     ═══════════════════════════════════════════════════════ -->

<section class="split compact">
  <div class="split-text">
    <div class="label">Standard Library</div>
    <h2><span class="paren">(</span>450+ builtins<span class="paren">)</span></h2>
    <p>
      Linked lists, vectors, and ordered maps with a full suite of
      higher-order operations. Slash-namespaced string functions, file I/O,
      HTTP client, JSON, regex, shell access, and more.
    </p>
    <p>
      Keywords in function position act as map accessors.
      Map bodies auto-serialize as JSON in HTTP requests.
    </p>
  </div>
  <div class="split-code">
<pre><code><span class="c">;; Collections</span>
<span class="p">(</span><span class="b">map</span> <span class="b">+</span> '<span class="p">(</span><span class="n">1</span> <span class="n">2</span> <span class="n">3</span><span class="p">)</span> '<span class="p">(</span><span class="n">10</span> <span class="n">20</span> <span class="n">30</span><span class="p">))</span>  <span class="c">; =&gt; (11 22 33)</span>
<span class="p">(</span><span class="b">filter</span> <span class="b">even?</span> <span class="p">(</span><span class="b">range</span> <span class="n">1</span> <span class="n">11</span><span class="p">))</span>   <span class="c">; =&gt; (2 4 6 8 10)</span>
<span class="p">(</span><span class="k">define</span> m <span class="p">{</span><span class="kw">:a</span> <span class="n">1</span> <span class="kw">:b</span> <span class="n">2</span> <span class="kw">:c</span> <span class="n">3</span><span class="p">})</span>
<span class="p">(</span><span class="b">assoc</span> m <span class="kw">:d</span> <span class="n">4</span><span class="p">)</span>               <span class="c">; =&gt; {:a 1 :b 2 :c 3 :d 4}</span>
<span class="p">(</span><span class="b">map/select-keys</span> m '<span class="p">(</span><span class="kw">:a</span> <span class="kw">:c</span><span class="p">))</span> <span class="c">; =&gt; {:a 1 :c 3}</span>

<span class="c">;; Strings</span>
<span class="p">(</span><span class="b">string/split</span> <span class="s">"a,b,c"</span> <span class="s">","</span><span class="p">)</span>     <span class="c">; =&gt; ("a" "b" "c")</span>
<span class="p">(</span><span class="b">string/join</span> '<span class="p">(</span><span class="s">"a"</span> <span class="s">"b"</span><span class="p">)</span> <span class="s">", "</span><span class="p">)</span> <span class="c">; =&gt; "a, b"</span>
<span class="p">(</span><span class="b">string/upper</span> <span class="s">"hello"</span><span class="p">)</span>        <span class="c">; =&gt; "HELLO"</span>

<span class="c">;; Files, HTTP &amp; JSON</span>
<span class="p">(</span><span class="b">file/write</span> <span class="s">"out.txt"</span> <span class="s">"hello"</span><span class="p">)</span>
<span class="p">(</span><span class="b">file/read</span> <span class="s">"out.txt"</span><span class="p">)</span>          <span class="c">; =&gt; "hello"</span>
<span class="p">(</span><span class="k">define</span> resp <span class="p">(</span><span class="b">http/get</span> <span class="s">"https://api.example.com/data"</span><span class="p">))</span>
<span class="p">(</span><span class="b">json/decode</span> <span class="p">(</span><span class="kw">:body</span> resp<span class="p">))</span>    <span class="c">; =&gt; {:key "val"}</span>
<span class="p">(</span><span class="b">shell</span> <span class="s">"ls -la"</span><span class="p">)</span>              <span class="c">; =&gt; {:exit-code 0 :stdout "..."}</span></code></pre>
  </div>
</section>

<!-- ——— Stdlib reference CTA ——— -->
<div class="ref-section" style="text-align: center;">
  <h3>Function Reference</h3>
  <p style="font-size: 1.15rem; color: var(--text); margin-bottom: 1.5rem; font-style: italic;">
    450+ builtins across 19 modules — math, strings, lists, maps, I/O, HTTP, regex, text processing, and more.
  </p>
  <a href="/docs/stdlib/" class="docs-link">Browse Standard Library Reference →</a>
</div>

<!-- ═══════════════════════════════════════════════════════
     FEATURES / ARCHITECTURE / INSTALL
     ═══════════════════════════════════════════════════════ -->

<section id="features" class="features-section">
  <h2><span class="paren">(</span>Features<span class="paren">)</span></h2>
  <div class="features-grid">
    <div class="feat">
      <h3>Tail-Call Optimized</h3>
      <p>Trampoline-based evaluator. Deep recursion without stack overflow.</p>
    </div>
    <div class="feat">
      <h3>Comprehensive Builtins</h3>
      <p>I/O, HTTP, regex, JSON, crypto, CSV, datetime, math, and more.</p>
    </div>
    <div class="feat">
      <h3>Tool Use &amp; Agents</h3>
      <p><code>deftool</code> and <code>defagent</code> as native special forms with multi-turn loops.</p>
    </div>
    <div class="feat">
      <h3>Structured Extraction</h3>
      <p>Define a schema as a map, get typed data back. <code>llm/extract</code> + <code>llm/classify</code>.</p>
    </div>
    <div class="feat">
      <h3>Streaming &amp; Batching</h3>
      <p>Real-time token streaming with <code>llm/stream</code>. Parallel batch with <code>llm/pmap</code>.</p>
    </div>
    <div class="feat">
      <h3>Persistent Conversations</h3>
      <p>Conversations are immutable values. Fork, extend, inspect message history as data.</p>
    </div>
    <div class="feat">
      <h3>Module System</h3>
      <p>File-based modules with <code>import</code> and <code>export</code>. Paths resolve relative to current file.</p>
    </div>
    <div class="feat">
      <h3>Error Handling</h3>
      <p><code>try</code> / <code>catch</code> / <code>throw</code> with typed error maps and full stack traces.</p>
    </div>
    <div class="feat">
      <h3>Bytecode VM</h3>
      <p>Sema compiles to bytecode and runs on a stack-based VM.</p>
    </div>
    <div class="feat">
      <h3>Standalone Executables</h3>
      <p><code>sema build</code> compiles programs into self-contained binaries with auto-traced imports and bundled assets.</p>
    </div>
    <div class="feat">
      <h3>Caching &amp; Resilience</h3>
      <p>Response caching, cost budgets, rate limiting, fallback chains, and retry with exponential backoff.</p>
    </div>
    <div class="feat">
      <h3>Vector Store</h3>
      <p>In-memory vector store with similarity search, cosine distance, and disk persistence.</p>
    </div>
    <div class="feat">
      <h3>Capability Sandbox</h3>
      <p><code>--sandbox</code> restricts shell, filesystem, network, and LLM access per capability group.</p>
    </div>
    <div class="feat">
      <h3>11 LLM Providers</h3>
      <p>Anthropic, OpenAI, Gemini, Ollama, Groq, xAI, Mistral, Moonshot, Jina, Voyage, Cohere.</p>
    </div>
    <div class="feat">
      <h3>Macros</h3>
      <p><code>defmacro</code> with quasiquote/unquote/splicing. <code>macroexpand</code> for inspection.</p>
    </div>
    <div class="feat">
      <h3>Clojure-Style Data</h3>
      <p>Keywords (<code>:foo</code>), maps (<code>{:k v}</code>), vectors (<code>[1 2]</code>). Keywords as functions.</p>
    </div>
    <div class="feat">
      <h3>Text Processing</h3>
      <p>Chunking, sentence splitting, HTML stripping, prompt templates, and document abstractions.</p>
    </div>
  </div>
</section>

<!-- ——— Architecture ——— -->
<section id="architecture" class="arch-section">
  <h2><span class="paren">(</span>Architecture<span class="paren">)</span></h2>
  <p>
    Eight Rust crates, one directed dependency graph. No circular dependencies.
    Single-threaded with <code class="ic">Rc</code>,
    deterministic ordering with <code class="ic">BTreeMap</code>.
  </p>

  <div class="crate-grid">
    <div class="crate-card">
      <h4>sema-core</h4>
      <p>Value types, environment, errors</p>
    </div>
    <div class="crate-card">
      <h4>sema-reader</h4>
      <p>Lexer and s-expression parser</p>
    </div>
    <div class="crate-card">
      <h4>sema-vm</h4>
      <p>Bytecode compiler, resolver, stack-based VM</p>
    </div>
    <div class="crate-card">
      <h4>sema-eval</h4>
      <p>Trampoline evaluator, special forms, modules</p>
    </div>
    <div class="crate-card">
      <h4>sema-stdlib</h4>
      <p>450+ builtins across 19 modules</p>
    </div>
    <div class="crate-card">
      <h4>sema-llm</h4>
      <p>LLM providers, vector store, caching, resilience</p>
    </div>
    <div class="crate-card">
      <h4>sema</h4>
      <p>REPL, CLI, file runner</p>
    </div>
    <div class="crate-card">
      <h4>sema-wasm</h4>
      <p>WebAssembly bindings for the browser playground</p>
    </div>
  </div>

  <div class="arch-diagram">
<pre><code>              sema-core
            /     |     \
    sema-reader   |   sema-stdlib
        |     \   |   /
    sema-vm  sema-eval    sema-llm
           \     |       /      \
               sema           sema-wasm</code></pre>
  </div>
</section>

<!-- ——— Install ——— -->
<section id="install" class="install-section">
  <h2><span class="paren">(</span>Install<span class="paren">)</span></h2>
  <p>Get running in one command.</p>

  <div class="install-methods-col">
    <div class="install-card">
      <h3>Homebrew</h3>
<pre><code><span class="prompt">$</span> brew install helgesverre/tap/sema-lang</code></pre>
    </div>
    <div class="install-card">
      <h3>Shell (macOS / Linux)</h3>
<pre><code><span class="prompt">$</span> curl -fsSL https://sema-lang.com/install.sh | sh</code></pre>
    </div>
    <div class="install-card">
      <h3>Cargo</h3>
<pre><code><span class="prompt">$</span> cargo install sema-lang</code></pre>
    </div>
    <div class="install-card">
      <h3>From source</h3>
<pre><code><span class="prompt">$</span> git clone https://github.com/sema-lisp/sema
<span class="prompt">$</span> cd sema && cargo build --release</code></pre>
    </div>
  </div>

  <div class="usage-block">
    <h3>Usage</h3>
<pre><code><span class="prompt">$</span> sema                              <span class="comment"># Start the REPL</span>
<span class="prompt">$</span> sema script.sema                  <span class="comment"># Run a file</span>
<span class="prompt">$</span> sema -e <span class="s">'(+ 1 2)'</span>                 <span class="comment"># Eval expression</span>
<span class="prompt">$</span> sema -p <span class="s">'(filter even? (range 10))'</span> <span class="comment"># Eval &amp; print</span>
<span class="prompt">$</span> sema -l prelude.sema script.sema  <span class="comment"># Load then run</span>
<span class="prompt">$</span> sema compile script.sema          <span class="comment"># Compile to .semac bytecode</span>
<span class="prompt">$</span> sema script.semac                 <span class="comment"># Run compiled bytecode</span>
<span class="prompt">$</span> sema build app.sema -o myapp      <span class="comment"># Build standalone executable</span>
<span class="prompt">$</span> sema disasm script.semac          <span class="comment"># Disassemble bytecode</span>
<span class="prompt">$</span> sema --sandbox=strict script.sema <span class="comment"># Sandboxed execution</span>
<span class="prompt">$</span> sema --no-llm script.sema         <span class="comment"># No LLM features</span></code></pre>
  </div>
</section>

<!-- ——— Footer ——— -->
<footer>
  <p>MIT &mdash; <a href="https://github.com/sema-lisp/sema">source</a></p>
</footer>

</div>
</template>

<style scoped>
  .custom-home {
    --bg: #0c0c0c;
    --bg-raised: #141414;
    --bg-code: #0a0a0a;
    --border: #222;
    --border-accent: #2a2520;
    --text: #d8d0c0;
    --text-dim: #7a7468;
    --text-bright: #f0ebe0;
    --gold: #c8a855;
    --gold-dim: #a08838;
    --green: #8aaa6a;
    --orange: #d08a60;
    --blue: #88a8c8;
    --purple: #b898c8;
    --serif: 'Cormorant', 'Georgia', serif;
    --mono: 'JetBrains Mono', monospace;
    font-family: var(--serif);
    background: var(--bg);
    color: var(--text);
    -webkit-font-smoothing: antialiased;
  }

  /* Reset VitePress defaults inside custom home */
  .custom-home a { color: inherit; text-decoration: none; }
  .custom-home h1, .custom-home h2, .custom-home h3, .custom-home h4 { border: none; margin: 0; }
  .custom-home p { margin: 0; }
  .custom-home pre { margin: 0; padding: 0; background: none; }
  .custom-home code { background: none; border-radius: 0; padding: 0; font-size: inherit; color: inherit; }

  /* ——— Hero: split layout ——— */
  .hero-split {
    display: grid;
    grid-template-columns: 1fr 1fr;
    min-height: 100vh;
    border-bottom: 1px solid var(--border);
  }

  .hero-text {
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding: 4rem 4rem 4rem 6rem;
  }

  .hero-headline {
    font-size: clamp(2.4rem, 4.5vw, 3.8rem);
    font-weight: 300;
    color: var(--text-bright);
    line-height: 1.15;
    letter-spacing: 0.01em;
    margin-bottom: 1.5rem;
  }

  .hero-accent {
    color: var(--gold);
  }

  p.hero-sub {
    font-size: 1.2rem;
    line-height: 1.75;
    color: var(--text);
    max-width: 30em;
    margin-bottom: 2rem;
  }

  .hero-actions {
    display: flex;
    gap: 1rem;
    margin-bottom: 2rem;
  }

  .hero-btn {
    font-family: var(--mono);
    font-size: 0.85rem;
    padding: 0.9rem 2.2rem;
    border-radius: 6px;
    text-decoration: none;
    letter-spacing: 0.04em;
    transition: background 0.2s, border-color 0.2s, color 0.2s;
  }

  .hero-btn.primary {
    background: var(--gold);
    color: #0c0c0c;
    border: 1px solid var(--gold);
    font-weight: 500;
  }

  .hero-btn.primary:hover {
    background: var(--gold-dim);
    border-color: var(--gold-dim);
  }

  .hero-btn.secondary {
    background: transparent;
    color: var(--text-bright);
    border: 1px solid var(--border);
  }

  .hero-btn.secondary:hover {
    border-color: var(--text-dim);
    color: var(--gold);
  }

  .hero-install-inline {
    font-family: var(--mono);
    font-size: 0.78rem;
    color: var(--text-dim);
  }

  .hero-install-inline .prompt {
    color: var(--gold-dim);
  }

  .hero-code {
    background: var(--bg-code);
    display: flex;
    align-items: center;
    padding: 3rem;
    border-left: 1px solid var(--border);
    overflow-x: auto;
  }

  .hero-code pre {
    width: 100%;
    line-height: 1.7;
    background: none;
    border: none;
    padding: 0;
    margin: 0;
  }

  .hero-code code {
    font-family: var(--mono);
    font-size: 0.85rem;
  }

  /* ——— Split sections: text | code side-by-side ——— */
  .split {
    display: grid;
    grid-template-columns: 1fr 1fr;
    min-height: 80vh;
    border-bottom: 1px solid var(--border);
  }

  .split.compact { min-height: auto; }

  .split.reverse { direction: rtl; }
  .split.reverse > * { direction: ltr; }

  .split-text {
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding: 4rem 4rem 4rem 6rem;
  }

  .split.reverse .split-text {
    padding: 4rem 6rem 4rem 4rem;
  }

  .split-text h2 {
    font-size: 2.8rem;
    font-weight: 300;
    color: var(--text-bright);
    margin-bottom: 0.8rem;
    letter-spacing: 0.02em;
  }

  .split-text h2 .paren {
    color: var(--text-dim);
    font-weight: 300;
  }

  .split-text p {
    font-size: 1.2rem;
    line-height: 1.75;
    color: var(--text);
    margin-bottom: 1rem;
  }

  .split-text .label {
    font-family: var(--mono);
    font-size: 0.7rem;
    color: var(--gold);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    margin-bottom: 1rem;
  }

  .split-text ul {
    list-style: none;
    margin-bottom: 1rem;
  }

  .split-text ul li {
    font-size: 1.1rem;
    line-height: 1.7;
    color: var(--text);
    padding-left: 1.2em;
    position: relative;
    margin-bottom: 0.3em;
  }

  .split-text ul li::before {
    content: "·";
    position: absolute;
    left: 0;
    color: var(--gold-dim);
    font-weight: 600;
  }

  .split-code {
    background: var(--bg-code);
    display: flex;
    align-items: center;
    padding: 3rem;
    border-left: 1px solid var(--border);
    overflow-x: auto;
  }

  .split.reverse .split-code {
    border-left: none;
    border-right: 1px solid var(--border);
  }

  .split-code pre {
    width: 100%;
    line-height: 1.6;
    background: none;
    border: none;
    padding: 0;
    margin: 0;
  }

  .split-code code {
    font-family: var(--mono);
    font-size: 0.82rem;
  }

  /* ——— Syntax colors ——— */
  .c  { color: #5a5a4a; font-style: italic; }
  .k  { color: #d4a052; }
  .s  { color: #8aaa6a; }
  .n  { color: #d08a60; }
  .kw { color: #c89050; }
  .b  { color: #88a8b8; }
  .p  { color: #444438; }
  .v  { color: #b898c8; }

  /* ——— Section divider with title ——— */
  .section-divider {
    padding: 5rem 4rem 2rem;
    text-align: center;
    border-bottom: 1px solid var(--border);
  }

  .section-divider h2 {
    font-size: 2.8rem;
    font-weight: 300;
    color: var(--text-bright);
    letter-spacing: 0.02em;
    margin-bottom: 0.5rem;
  }

  .section-divider h2 .paren { color: var(--text-dim); }

  .section-divider p {
    font-size: 1.15rem;
    color: var(--text-dim);
    font-style: italic;
    max-width: 36em;
    margin: 0 auto;
    line-height: 1.6;
  }

  /* ——— Reference tables ——— */
  .ref-section {
    padding: 4rem;
    border-bottom: 1px solid var(--border);
    max-width: 72rem;
    margin: 0 auto;
  }

  .ref-section h3 {
    font-family: var(--mono);
    font-size: 0.75rem;
    color: var(--gold);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 1.5rem;
  }

  .type-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(15rem, 1fr));
    gap: 0.8rem;
    margin-bottom: 2rem;
  }

  .type-card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem 1.2rem;
    display: flex;
    align-items: baseline;
    gap: 0.8rem;
  }

  .type-card .type-name {
    font-family: var(--mono);
    font-size: 0.75rem;
    color: var(--gold);
    white-space: nowrap;
    min-width: 5rem;
  }

  .type-card .type-ex {
    font-family: var(--mono);
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  /* ——— Stdlib category grid ——— */
  .stdlib-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(20rem, 1fr));
    gap: 1.2rem;
  }

  .stdlib-card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.5rem;
  }

  .stdlib-card h4 {
    font-family: var(--mono);
    font-size: 0.72rem;
    color: var(--gold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    margin-bottom: 0.8rem;
  }

  .stdlib-card .fn-list {
    font-family: var(--mono);
    font-size: 0.68rem;
    color: var(--text-dim);
    line-height: 1.9;
    word-break: break-word;
  }

  .stdlib-card .fn-list span {
    display: inline-block;
    background: var(--bg-code);
    border: 1px solid var(--border);
    padding: 0.1em 0.45em;
    border-radius: 3px;
    margin: 0.1em 0.15em;
    white-space: nowrap;
  }

  /* ——— Features grid (full-width section) ——— */
  .features-section {
    padding: 6rem 4rem;
    border-bottom: 1px solid var(--border);
  }

  .features-section h2 {
    font-size: 2.8rem;
    font-weight: 300;
    color: var(--text-bright);
    text-align: center;
    margin-bottom: 3rem;
  }

  .features-section h2 .paren { color: var(--text-dim); }

  .features-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 1.5rem;
    max-width: 72rem;
    margin: 0 auto;
  }

  .feat {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2rem;
    transition: border-color 0.2s;
  }

  .feat:hover { border-color: var(--border-accent); }

  .feat h3 {
    font-family: var(--mono);
    font-size: 0.75rem;
    color: var(--gold);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 0.6rem;
  }

  .feat p {
    font-family: var(--serif);
    font-size: 1.15rem;
    line-height: 1.6;
    color: var(--text);
  }

  .feat code {
    font-family: var(--mono);
    font-size: 0.78rem;
    color: var(--gold-dim);
    background: var(--bg-code);
    border: 1px solid var(--border);
    padding: 0.1em 0.4em;
    border-radius: 3px;
  }

  /* ——— Providers strip ——— */
  .providers-strip {
    padding: 3rem 4rem;
    border-bottom: 1px solid var(--border);
    text-align: center;
  }

  .providers-strip h3 {
    font-family: var(--mono);
    font-size: 0.7rem;
    color: var(--text-dim);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    margin-bottom: 1.2rem;
  }

  .provider-list {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.8rem;
    max-width: 52rem;
    margin: 0 auto;
  }

  .provider-list span {
    font-family: var(--mono);
    font-size: 0.72rem;
    padding: 0.4em 1em;
    border: 1px solid var(--border);
    border-radius: 20px;
    color: var(--text);
    background: var(--bg-raised);
  }

  /* ——— Architecture section ——— */
  .arch-section {
    padding: 6rem 4rem;
    border-bottom: 1px solid var(--border);
    max-width: 64rem;
    margin: 0 auto;
  }

  .arch-section h2 {
    font-size: 2.8rem;
    font-weight: 300;
    color: var(--text-bright);
    text-align: center;
    margin-bottom: 1rem;
  }

  .arch-section h2 .paren { color: var(--text-dim); }

  .arch-section > p {
    font-size: 1.15rem;
    line-height: 1.7;
    color: var(--text);
    text-align: center;
    max-width: 40em;
    margin: 0 auto 2.5rem;
  }

  .crate-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 1rem;
    margin-bottom: 2rem;
  }

  .crate-card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.4rem;
  }

  .crate-card h4 {
    font-family: var(--mono);
    font-size: 0.78rem;
    color: var(--gold);
    margin-bottom: 0.3rem;
  }

  .crate-card p {
    font-size: 1rem;
    line-height: 1.5;
    color: var(--text-dim);
    margin: 0;
  }

  .arch-diagram {
    background: var(--bg-code);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2rem;
    text-align: center;
  }

  .arch-diagram pre {
    display: inline-block;
    text-align: left;
    line-height: 1.5;
    background: none;
    border: none;
    padding: 0;
    margin: 0;
  }

  .arch-diagram code {
    font-family: var(--mono);
    font-size: 0.82rem;
    color: var(--text-dim);
  }

  /* ——— Install section ——— */
  .install-section {
    padding: 6rem 4rem;
    text-align: center;
    border-bottom: 1px solid var(--border);
  }

  .install-section h2 {
    font-size: 2.8rem;
    font-weight: 300;
    color: var(--text-bright);
    margin-bottom: 0.5rem;
  }

  .install-section h2 .paren { color: var(--text-dim); }

  .install-section > p {
    font-size: 1.15rem;
    color: var(--text-dim);
    font-style: italic;
    margin-bottom: 2rem;
  }

  .install-methods-col {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    max-width: 52rem;
    margin: 0 auto 2.5rem;
  }

  .install-card {
    background: var(--bg-code);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2rem;
    text-align: left;
  }

  .install-card h3 {
    font-family: var(--mono);
    font-size: 0.7rem;
    color: var(--gold);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    margin-bottom: 1rem;
  }

  .install-card pre {
    background: none;
    border: none;
    padding: 0;
    margin: 0;
    line-height: 1.7;
  }

  .install-card code {
    font-family: var(--mono);
    font-size: 0.8rem;
    color: var(--text-bright);
  }

  .install-card .prompt { color: var(--gold); }
  .install-card .comment { color: #5a5a4a; }

  .usage-block {
    max-width: 52rem;
    margin: 0 auto;
  }

  .usage-block h3 {
    font-family: var(--mono);
    font-size: 0.7rem;
    color: var(--gold);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    margin-bottom: 1rem;
  }

  .usage-block pre {
    background: var(--bg-code);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.6rem 2rem;
    text-align: left;
    line-height: 1.7;
  }

  .usage-block code {
    font-family: var(--mono);
    font-size: 0.8rem;
    color: var(--text-bright);
  }

  .usage-block .prompt { color: var(--gold); }
  .usage-block .comment { color: #5a5a4a; }
  .usage-block .output { color: var(--text-dim); }

  /* ——— Footer ——— */
  footer {
    text-align: center;
    padding: 3rem 2rem;
    color: var(--text-dim);
    font-size: 1rem;
    font-style: italic;
  }

  footer a { color: var(--text-dim); text-decoration: underline; }
  footer a:hover { color: var(--gold); }

  /* ——— Inline code in prose ——— */
  .custom-home code.ic {
    font-family: var(--mono);
    font-size: 0.88em;
    color: var(--gold-dim);
  }

  /* ——— Responsive ——— */
  @media (max-width: 900px) {
    .hero-split {
      grid-template-columns: 1fr;
      min-height: auto;
    }
    .hero-text { padding: 3rem 2rem; }
    .hero-headline { font-size: clamp(2rem, 7vw, 3rem); }
    .hero-code {
      border-left: none;
      border-top: 1px solid var(--border);
      padding: 2rem;
    }
    .hero-actions { flex-wrap: wrap; }
    .split {
      grid-template-columns: 1fr;
      min-height: auto;
    }
    .split.reverse { direction: ltr; }
    .split-text { padding: 3rem 2rem; }
    .split.reverse .split-text { padding: 3rem 2rem; }
    .split-code {
      border-left: none;
      border-top: 1px solid var(--border);
      padding: 2rem;
    }
    .split.reverse .split-code { border-right: none; border-top: 1px solid var(--border); }
    .features-grid { grid-template-columns: 1fr; }
    .crate-grid { grid-template-columns: 1fr; }
    .install-methods-col { gap: 0.75rem; }
    .features-section, .arch-section, .install-section, .providers-strip { padding: 3rem 1.5rem; }
    .ref-section { padding: 3rem 1.5rem; }
    .section-divider { padding: 3rem 1.5rem 1.5rem; }
    .stdlib-grid { grid-template-columns: 1fr; }
  }

.docs-link {
  display: inline-block;
  font-family: var(--mono);
  font-size: 0.85rem;
  color: var(--gold);
  border: 1px solid var(--gold-dim);
  padding: 0.8rem 2rem;
  border-radius: 6px;
  text-decoration: none;
  transition: background 0.2s, border-color 0.2s;
}
.docs-link:hover {
  background: rgba(200, 168, 85, 0.1);
  border-color: var(--gold);
}
</style>
