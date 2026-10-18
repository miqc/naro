# Relatório — Sprint 13: validação em páginas reais

Medido em 2026-10-18, host Windows, build `--release` (lto=fat, cgu=1, panic=abort, strip).

## Páginas testadas

| # | Página | Tipo | Navegação | Título | `CONTENT_HEIGHT` | JS |
|---|---|---|---|---|---|---|
| 1 | <https://example.com> | estática | ok | Example Domain | 172,8 | — |
| 2 | <https://www.rust-lang.org> | documentação, CSS externo | ok | Rust Programming Language | 4333,3 | — |
| 3 | <https://developer.mozilla.org/en-US/docs/Web/HTML> | dinâmica moderada | ok | HTML: HyperText Markup Language \| MDN | 17190,0 | 1 erro |

Capturas: `reports/example_com.png`, `reports/www_rust-lang_org.png`,
`reports/developer_mozilla_org_en-US_docs_Web_HTML.png` (antes das correções),
`reports/mdn2/3/4.png` (após cada correção).

## Estado antes das correções

### example.com — funcional

CSS inline aplicado (`max-width:26em`, centralizado, `text-align:center`),
texto correto, sem erros. Nada a corrigir.

### rust-lang.org — parcial

CSS externo carregado, nav horizontal (flex), faixa verde, textos e imagens.
Limitações visíveis: layout em coluna única porque o site usa grid.

### MDN — ruim

- **17 folhas de estilo**, das quais só **6** eram baixadas (`MAX_SHEETS`).
  As folhas que definiam tipografia, cores e o layout da referência ficavam de fora.
- Todas as cores vinham de **custom properties** (`var(--color-text-primary)`),
  que o parser não conhecia: declarações descartadas como inválidas.
- Seletores com **pseudo-classes** (`:root`, `a:hover`, `li:last-child`) eram
  rejeitados inteiros — inclusive `:root { --palette… }`, que é onde o site
  define as variáveis.
- Resultado: lista vertical sem estilo, sobreposição de texto na seção
  "Markup languages" (conteúdo de duas colunas empilhado na mesma linha).
- 1 erro JS: `TypeError: not a function` no primeiro script inline
  (API do navegador ausente — não investigado nesta sprint).

## Correções realizadas

| # | Correção | Arquivo | Efeito |
|---|---|---|---|
| 1 | Orçamento de folhas: 6 → **12 folhas**, com teto total de **3 MiB** | `naro-net` | MDN passou a receber as folhas de tipografia e de layout da referência |
| 2 | **Custom properties**: coleta global de `--nome: valor` + substituição recursiva de `var(--x, fallback)` em toda declaração | `naro-css` | Cores, espaçamentos e fontes definidos por variável passam a valer |
| 3 | **Pseudo-classes simples** em seletores: `a:hover` → `a`, `:root` → `html`; atributo e funcionais continuam rejeitados | `naro-css` | Regras antes descartadas por inteiro passam a aplicar |
| 3b | **`list-style: none`** respeitado (antes todo `<li>` recebia bullet) | `naro-css`, `naro-layout` | Nav do rust-lang.org deixa de mostrar `•` que o site remove |

Teste novo: `custom_properties_are_substituted` (variável definida, herdada e com fallback).

## Resultado após as correções

| Página | Antes | Depois |
|---|---|---|
| example.com | ok | ok (`CONTENT_HEIGHT` 160,8 → 172,8 pela regra `li` do UA) |
| rust-lang.org | 4335,3 | 4333,3 (estável) |
| MDN | 16347,6, sem estilo | 17190,0, **texto com cores e tipografia do site**, links azuis, cabeçalho e listas estilizados |

Observação: a nav do rust-lang.org perdeu também os separadores `•` entre
itens. Eles vêm de `li::before { content }`, que não é suportado — a remoção
dos bullets foi correta, a perda dos separadores é limitação.

O MDN continua **sem o layout de três colunas**: `.page-layout` usa
`display: grid`, e grid não é suportado. Isso é limitação conhecida da engine,
não defeito introduzido.

## Testes

- `cargo test --workspace`: **45 passando, 0 falhas**.
- `naro --compat tests/compat`: **13 PASS, 0 FAIL, 1 UNSUP (total 14)** —
  idêntico ao baseline medido antes das mudanças.

## Separação de problemas

**Rede:** nenhum. As três páginas responderam 200 e o TLS do Schannel funcionou.

**Limitações conhecidas da engine (não corrigidas):**

- `display: grid` e `position: sticky/absolute` — responsáveis pelo layout
  em coluna única e pela sobreposição no MDN.
- Módulos ES, `fetch`, `Image`, `IntersectionObserver` — o MDN depende de
  alguns; o erro `not a function` vem daí.
- `calc()`, `clamp()`, `min()/max()`, `rem`/`vh`/`svh` — muitos valores de
  espaçamento das folhas modernas continuam descartados.
- `box-sizing`, `float`, `min-width`, `overflow` não implementados.

**Defeitos novos:** nenhum. As duas mudanças de comportamento observadas
(`example.com` 160,8 → 172,8) vêm da regra `li { margin: 4px 0 }` do UA
stylesheet, que antes não alcançava os itens e agora alcança — mudança
esperada e verificada nas capturas.

## Disco

155 MB livres no início · 148 MB ao final (1 build de release + 4 renders).
Nenhum diretório de build foi removido.
