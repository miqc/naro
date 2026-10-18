# Estabilização do layout — cinco lacunas, cinco regressões protegidas

Data: 2026-10-18
Escopo: apenas as cinco lacunas de layout encontradas ao investigar o Google. Nenhuma
fase do roadmap foi iniciada. O Google é smoke test, não critério de aceitação.

## 1. Casos adicionados

| # | Lacuna | Teste | Arquivo |
|---|---|---|---|
| 1 | `flex_width` vaza após `take()` + early returns | `flex_item_width_does_not_leak_into_the_next_item` | `crates/naro-layout/src/lib.rs` |
| 2 | `<svg>` sem dimensões ocupa espaço indevido | `unsized_svg_fills_its_wrapper_and_nothing_else` | idem |
| 3 | `inline` com `width` tratado como caixa | `width_on_a_non_replaced_inline_box_is_ignored` | idem |
| 4 | flex/grid ignoram a própria largura | `flex_and_grid_containers_honour_their_own_width` | idem |
| 5 | container flex sem largura medido errado (`UMOYhd`) | `auto_width_flex_row_shrink_wraps_around_a_sized_child` | idem |

Página de compatibilidade nova: `tests/compat/27-layout-sizing.html`, com check novo
`box:<seletor>:<w>x<h>` (tamanho exato do primeiro bloco do seletor, não "o primeiro div
da página").

## 2. Causas confirmadas

Cada causa foi confirmada por instrumentação temporária (`eprintln!` em `block`,
`max_content_width`, `min_content_width`), removida depois, e por medição de referência
no Chromium (`getBoundingClientRect`), não por suposição.

### 2.1 Vazamento de `flex_width`

`block()` fazia `self.flex_width.take()` **depois** dos early returns de `svg`, `flex`,
`table`, `grid`. Um item flex cujo layout saía por um desses caminhos deixava o valor
para o próximo `block()` da árvore. Medido: `div#col` (coluna, 400px) entregava
`flex_width = Some(100)` para `#a`; `#a` era `display:flex` e saía sem consumir; o valor
caía em `#b`. Correção: `take()` na primeira linha de `block()`.

### 2.2 `<svg>` sem dimensões

Duas causas distintas, ambas medidas:

1. `width: 100%` era resolvido contra a largura oferecida (a página), não contra o
   wrapper. Um ícone de 960 unidades do `viewBox` virava 678×678 e cobria o campo de
   busca. Correção: `100%` resolve contra o content box do container (o wrapper tem
   largura definida, então o valor é 24).
2. Sem `width` nenhum, o fallback era o `viewBox` em unidades de usuário. Chrome mede
   esse caso como **o tamanho do wrapper** (SVG2: `width: auto` computa para `100%`).
   Correção: `(None, None) => available.max(1.0)`, altura pelo ratio do `viewBox`.

Verificado no Chromium: `div 24×24 > svg viewBox 960` → svg 24×24.

### 2.3 `inline` com `width`

Uma tentativa anterior (minha) boxificava `display:inline` com `width`/`height` para dar
um container ao SVG. Está **errado** segundo CSS e segundo o Chromium: `width` é
ignorado em caixa inline não substituída. Medição de referência: `span.pill{display:inline;
width:60px;height:20px}` → **8×17** (só o glifo), não 60×20. A boxificação foi revertida e
o teste fixa o comportamento correto.

### 2.4 flex/grid ignorando a própria largura

`flex()` e `grid()` calculavam `content_w` só a partir da largura oferecida, sem aplicar
`width`/`min-width`/`max-width` do próprio container. Um `button` de 36px em `display:flex`
virava 678px. Correção: helper `box_content_width()` usado por `block`, `flex` e `grid`.

### 2.5 `UMOYhd` — container flex sem largura

**Não era a mesma causa de 2.1–2.4.** A causa é a medição intrínseca: `max_content_width`
e `min_content_width` recursavam nos filhos mesmo quando o próprio elemento tinha
`width` absoluta definida. `span.wPSZBe{width:24px}` media o `<svg>` interno com
`width:100%`, e essa porcentagem resolvia contra a largura-sonda (1000px), devolvendo
960–1000. Resultado: `UMOYhd` (auto-width) media 48 de max-content mas **1000 de
min-content**, e o `min-width:auto` de flex item forçava a linha a 678px.

Correção: `definite_content_width()` — largura absoluta declarada encerra a medição
intrínseca. Medição depois: `MINCW UMOYhd = 48.0`, `MINCW wPSZBe = 24.0`, e o campo de
busca voltou a `x=205 w=417.5` com o wrapper de ícone em `48×24`.

## 3. Arquivos alterados

| Arquivo | Alteração |
|---|---|
| `crates/naro-layout/src/lib.rs` | `box_content_width()`, `definite_content_width()`, `intrinsic_atom_width()`; `take()` de `flex_width` no topo de `block()`; `svg_size()` reescrito; boxificação de inline revertida; 5 testes novos + helpers `build_doc`/`node_with_id`/`block_for` |
| `crates/naro-app/src/main.rs` | check `box:<seletor>:<w>x<h>` no runner de compatibilidade |
| `tests/compat/27-layout-sizing.html` | página nova |
| `tests/compat/manifest.txt` | linha de 27 + documentação do check `box` |
| `reports/` | este relatório, `g20.png` (Google), `rust-after.png`, `s27-sizing.png` |

## 4. Resultados

### Testes

```
cargo test --workspace
  naro-css     48 passed
  naro-engine  49 passed
  naro-html     7 passed
  naro-image    2 passed
  naro-js       4 passed
  naro-layout  83 passed   (78 antes; +5)
  naro-net      2 passed
  naro-paint    0 passed
  naro-text     3 passed
  ---------------------------------
  total       198 passed, 0 failed
```

Cada um dos cinco testes foi verificado nos dois sentidos: com a correção aplicada passa;
revertendo a correção correspondente, falha. Reversões testadas: `flex_width` de volta
para depois dos early returns (falha), `flex`/`grid` sem `box_content_width` (falha),
`intrinsic_atom_width` neutralizado + `definite_content_width` removido (falha),
fallback de svg para o `viewBox` (falha), boxificação de inline reintroduzida (falha).

### Compatibilidade

```
naro --compat tests/compat
  resumo: 26 pass, 0 falhas, 1 não suportados (total 27)
```

Baseline da sessão: 21 pass / 0 fail / 1 unsup (total 22). As cinco páginas a mais
(23–27) entram passando; nenhuma página anterior regrediu.

### Páginas reais

| Página | STATUS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|
| `naro://start` | Idle | 606,40 (baseline 606,40) | nenhum |
| `example.com` | Idle | 820,60 (baseline 172,80) | baseline tinha `s.js: not a function`; **agora nenhum** |
| `rust-lang.org` | Idle | 3968,98 (baseline 3233,16) | 0 → 0 |
| `google.com` | Idle | 487,90 | pré-existentes (8 scripts), nenhum novo |

`CONTENT_HEIGHT` maior é esperado e correto: os ícones deixaram de ser quadrados de
678–960px e o texto passou a quebrar em colunas normais. Em `example.com` o texto vinha
em faixas de 67px; agora quebra em 480px de largura, como no navegador.

`rust-lang.org` renderiza o hero legível ("A language empowering everyone to build
reliable and efficient software."), com o seletor de idiomas em uma linha — antes o texto
era esmagado em colunas estreitas. Nenhum `JS ERROR` novo.

### Google (smoke)

Renderiza sem travar, `STATUS: Idle`. Campo de busca clicável e focável:
`--hit 400 270` devolve `textarea.gLFyf rect=(205,261,417.5,44)`; `--form --click 400 270`
dá `foco: Some(157)`. Ícones de teclado, microfone, lente e "Modo IA" aparecem dentro do
campo, com o wrapper de 48×24.

## 5. Limitações reais

- **Google continua com diferenças visuais** e não é critério. Restam: ícone de lupa
  sobreposto ao texto de placeholder, ordem/tamanho de alguns elementos do header, e os
  8 erros de JS pré-existentes (`XMLHttpRequest is not defined`, `not a function`).
  Nada disso foi atacado nesta etapa, por instrução.
- **`example.com` melhorou por acidente**: a página serve conteúdo em árabe quando o
  `Accept-Language` é genérico; o `JS ERROR` que existia no baseline desapareceu porque o
  script que falhava é servido por outra variante regional. Não é correção de JS.
- **Cinco testes, não uma suíte.** Cada teste fixa um caso mínimo derivado de medição no
  Chromium. Lacunas adjacentes (por exemplo `min-width: auto` com porcentagem, tabelas com
  `colspan` em grid) não foram cobertas.
- **O check `box` só olha o primeiro bloco do seletor.** Para o caso 27 isso é suficiente
  porque cada seletor é único na página; não serve para listas.
- **`--compat` não distingue "cor ausente" de "regra não aplicada"** em todos os casos; a
  cobertura de cascata continua nos testes unitários.

## 6. O que não foi feito

- Nenhuma fase do roadmap (Fase 1 — seletores/cascata) foi iniciada.
- Nenhuma tentativa de aproximar o Google pixel a pixel.
- Nenhum subsistema fora de `naro-layout` (e do check de compatibilidade) foi alterado.
