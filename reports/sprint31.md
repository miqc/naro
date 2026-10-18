# Sprint 31 — Fase 8: otimização do parsing de CSS

Escopo: `naro-css` (parser) e o ponto do `naro-engine` que decide quando reparsear.
Nenhuma fase anterior foi reaberta, nenhuma fase futura iniciada, e a interface
gráfica não foi tocada.

## 1. Diagnóstico antes de alterar

### Caminho completo do CSS

```
documento → collect_style_text() (concatena todo <style>)
          → strip_comments()        (String nova)
          → strip_at_rules()        (String nova, recursiva p/ @media)
          → laço de regras: find('{') / find('}')
                ├─ parse_declarations(body)          → Vec<Declaration>
                └─ parse_selector_list_reporting()   → Vec<Selector>
          → Stylesheet { rules, variables, report }
```

`parse_stylesheet_for(src, width)` é chamado em `compute_page_styles`, que o
`naro-engine` usa em quatro pontos: passada provisória (para os scripts medirem),
passada final, restyle após mutação, e o `--bench`.

### Instrumentação (temporária, removida depois)

Medido em `rust-lang.org`, 106 619 bytes de CSS, 2 052 regras:

| Etapa | Tempo | Observação |
|---|---|---|
| `strip_comments` | 0,08 ms | |
| `strip_at_rules` (incremental) | 0,23 ms | |
| `parse_declarations` | 0,68 ms | 1 813 chamadas |
| **`parse_selector_list_reporting`** | **2,65 ms** | 1 813 chamadas |
| laço restante | ~0,98 ms | `find`, clones, HashMap |
| **total** | **4,7 ms** | |

O parsing de **seletores** era 56% do custo — não as declarações, e não o
`@media`. Isso contrariava a hipótese registrada no sprint29 ("o parser aloca uma
`String` por nome e por valor de declaração"), que apontava para o lugar errado.

### Causa

`parse_complex` e `parse_compound_part` faziam `text.chars().collect::<Vec<char>>()`
— uma alocação de heap por composto, e iteração por `char` com índice — e
`parse_complex` ainda acumulava o composto num `String` com `push` por caractere.
Com 1 813 seletores e múltiplos compostos cada, eram milhares de `Vec<char>` e
milhares de `push`.

### Reparseamento

Com um contador temporário: **2 parses por carga** (provisória + final), sempre do
mesmo texto, e **3 no Google** (mais um restyle). A passada provisória existe só
para dar geometria aos scripts e o seu `Stylesheet` é descartado.

## 2. Otimizações aplicadas

### 2.1 Parser de seletores sobre índices de byte

`parse_complex`, `parse_compound_part`, `parse_attr_selector` e `parse_pseudo`
passaram a operar em `&str` com offsets de byte. O composto corrente é a fatia
`text[start..end]`, sem buffer por caractere. Helpers novos: `char_at`,
`skip_space_at` e `take_name` (que devolve `&str` emprestado). A única alocação que
sobrou por nome é a `String` que o `Compound` de fato guarda.

**Medido (mesma build, mesma página, `sel_time`):** 2,65 ms → 1,18 ms.

### 2.2 `!important` sem copiar o valor

`parse_declarations` fazia `value.to_ascii_lowercase()` — uma cópia completa do
valor — em toda declaração, só para comparar o sufixo. Agora a comparação é
`eq_ignore_ascii_case` em fatia, com `value.truncate` no acerto. `!important` é
raro; a cópia era paga sempre.

### 2.3 Cache de folha por conteúdo (`SheetCache`)

```rust
pub struct SheetCache { last: RefCell<Option<(String, f32, Rc<Stylesheet>)>> }
```

A chave é o **texto exato** mais a **largura do viewport** — tudo de que
`parse_stylesheet_for` depende. Qualquer edição no texto é um *miss* e substitui a
entrada, então um acerto não pode devolver folha velha. Devolve `Rc<Stylesheet>`
para o acerto não custar um clone profundo das regras.

Efeito: a carga passa a parsear **uma vez** em vez de duas, e um restyle cujo
`<style>` não mudou não reparseia.

### 2.4 Correção de bug encontrada no caminho

`strip_at_rules` copiava o texto byte a byte com `out.push(bytes[i] as char)`. Para
qualquer byte >= 0x80 isso reinterpreta o byte como Latin-1: `.ação` virava
`.aÃ§Ã£o` antes do parser de seletores. **Todo CSS com caractere não-ASCII estava
corrompido** — classes, valores de atributo e `content: "…"`. Trocado por cópia de
fatia até o próximo `@` (também mais rápido). Coberto por
`selector_parsing_survives_multibyte_names` e verificado ponta a ponta:

```
p.ação        → color=#0a7d3f   (antes: não casava)
p[data-x="ção"] → color=#8a1f1f (antes: não casava)
```

## 3. Resultados

Metodologia: mesmo binário `--release`, mesma máquina, mediana de 3 execuções de 10
iterações por alvo. `css` mede **um parse frio** (o `--bench` não usa o cache), então
a coluna isola a velocidade do parser; `load` mede a carga real com cache.

| Alvo | fase | antes | depois | Δ |
|---|---|---|---|---|
| rust-lang.org | `css` | 4,77 ms | **2,90 ms** | **−39%** |
| rust-lang.org | alocações | 648 361 | **413 109** | **−36%** |
| MDN `/docs/Web/HTML` | `css` | 2,61 ms | **1,60 ms** | **−39%** |
| MDN | alocações | 831 262 | **733 343** | −12% |
| google.com | `css` | 4,79 ms | **3,38 ms** | −29% |
| google.com | alocações | 720 477 | **507 648** | **−30%** |
| `naro://start` | `css` | 0,024 ms | 0,017 ms | −29% |
| `naro://start` | alocações | 17 828 | 16 351 | −8% |
| example.com | alocações | 8 218 | 7 319 | −11% |

### Cache: A/B na mesma build

Interruptor de ambiente, 5 execuções de 10 iterações, mediana:

| Alvo | sem cache | com cache | Δ |
|---|---|---|---|
| rust-lang.org (`load`) | 18,87 ms | **14,84 ms** | **−21%** |
| MDN (`load`) | 43,42 ms | 41,19 ms | −5% |

A diferença entre os dois é o tamanho do `<style>`: 106 KB em rust-lang, poucos
KB em MDN (a maior parte do CSS do MDN vem em folhas externas que o motor não
parseia como `<style>`).

### Estado final por fase

| Alvo | parse | css | style | layout | load | alocações |
|---|---|---|---|---|---|---|
| `naro://start` | 0,011 | 0,017 | 0,063 | 0,043 | 3,57 | 16 351 |
| example.com | 0,006 | 0,007 | 0,031 | 0,013 | 3,64 | 7 319 |
| rust-lang.org | 0,42 | 2,91 | 2,15 | 0,95 | 13,43 | 413 109 |
| MDN | 1,48 | 1,60 | 12,50 | 5,34 | 40,02 | 733 343 |
| google.com | 0,91 | 3,38 | 3,07 | 0,62 | 32,16 | 507 469 |

## 4. Validação

```
cargo test --workspace     228 passed, 0 failed    (225 antes; +3)
naro --compat              27 pass, 0 falhas, 1 não suportados (total 28)
```

Testes novos:

- `sheet_cache_returns_the_same_rules_for_the_same_text` — mesmo texto e viewport
  devolvem o **mesmo** `Rc` (`ptr_eq`), não um reparse.
- `sheet_cache_misses_when_the_text_or_viewport_changes` — texto diferente e
  viewport diferente invalidam; o caso de viewport cobre `@media`.
- `selector_parsing_survives_multibyte_names` — nomes acima de U+007F.

Geometria e conteúdo inalterados:

| Página | STATUS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|
| example.com | Idle | 820,60 (idêntico) | 0 |
| rust-lang.org | Idle | 3968,98 (idêntico) | 0 |
| google.com | Idle | 487,90 (idêntico) | 8 pré-existentes |
| MDN | Idle | 16897,00 (idêntico) | 6 (idênticos ao baseline) |

`rust-lang.org` comparado bloco a bloco com o dump do sprint29: **142 blocos, 142
depois, 0 diferenças de geometria.**

Nota sobre `rules=`: o Google varia entre 1276 e 1280 regras entre requisições
(A/B do próprio site). Verificado com três buscas seguidas no mesmo binário — é
variação da página, não da fase.

## 5. Resultados negativos e o que não foi feito

- **`strip_comments` + `strip_at_rules` ainda alocam duas `String` do tamanho da
  folha** (~0,31 ms em rust-lang, ~11% do parse restante). Fundir as duas passadas
  numa só seria possível, mas o ganho estimado é menor que a variação de medição
  deste alvo; não foi feito sem A/B que o justifique.
- **`declarations.clone()` por seletor da lista** (2 052 regras a partir de 1 813
  blocos = ~239 clones). Trocar por `Rc<Vec<Declaration>>` mudaria `Rule`, que é
  API pública usada pelo layout. Sem medição que mostre ganho relevante, ficou.
- **Sem reparse é a única coisa que o cache evita.** Ele não evita recascata nem
  relayout; invalidação por subárvore continua fora de escopo, como a fase pedia.
- **O parse do CSS ainda é 3 ms em folhas grandes.** O parser aloca uma `String`
  por nome e por valor de declaração; eliminar isso exigiria um tipo de string
  emprestada com o tempo de vida da folha, o que é uma mudança de modelo, não uma
  otimização local.

## 6. Arquivos alterados

| Arquivo | Alteração |
|---|---|
| `crates/naro-css/src/lib.rs` | parser de seletores sobre índices de byte (`char_at`, `skip_space_at`, `take_name`); `!important` sem copiar o valor; `SheetCache`; correção UTF-8 em `strip_at_rules`; 3 testes |
| `crates/naro-engine/src/lib.rs` | `sheet_cache` no `Engine`; `compute_page_styles` recebe `&SheetCache` |
| `crates/naro-app/src/main.rs` | `--bench` imprime `load ms` (carga ponta a ponta, separada das fases) |

Nenhum relatório, captura ou benchmark anterior foi apagado. `reports/p8-rust.txt`
guarda o dump usado na comparação de geometria.

## 7. Oportunidades futuras (não iniciadas)

1. Fundir `strip_comments` e `strip_at_rules` numa passada com buffer reutilizado.
2. `Rc<Vec<Declaration>>` em `Rule`, se um perfil de memória justificar.
3. Strings emprestadas no `Declaration` (exigiria a folha viver mais que as regras).
4. Cascata: 12,5 ms no MDN continua sendo o maior custo isolado de estilo —
   aplicar declarações (parse de valores), não casar seletores.
