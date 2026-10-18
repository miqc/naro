# Sprint 29 — Fase 7: desempenho e memória

Escopo: Fase 7 do plano mestre. Nenhuma fase anterior foi reaberta; nenhuma correção
visual do Google foi tocada. O critério do plano — `--bench` antes/depois com mediana e
RSS da GUI — foi cumprido; o que não deu melhoria confiável está registrado como tal.

## 1. Metodologia

- Ferramenta: `naro --bench <alvo> <iters> [largura]`, que já existia. Mede mediana por
  fase, contagem de alocações (alocador global instrumentado) e pico/live do heap.
- **Uma mudança na ferramenta:** `style` media duas coisas diferentes — parse do CSS e
  cascata. Como a otimização principal atua só na cascata, a fase foi dividida em
  `css` (parse das folhas) e `style` (cascata). Isso é instrumentação, não mudança de
  critério: as duas continuam sendo medidas e somadas.
- Alvos: `naro://start` (página mínima embutida), `example.com`, `rust-lang.org`,
  MDN `/docs/Web/HTML`, `google.com`, mais dois fixtures salvos em `.tmp/bench/`
  (`rust.html`, `mdn.html`) para eliminar variação de rede entre execuções.
- Cada número abaixo é a **mediana de 3 execuções de 10–20 iterações**, na mesma máquina,
  com o mesmo binário `--release`. Onde há A/B, as duas metades rodaram na mesma build,
  com um interruptor de ambiente, para que o custo do interruptor fosse idêntico dos dois
  lados.

## 2. Baseline (antes de qualquer alteração)

| Alvo | parse | style (inclui parse CSS) | layout | paint | alocações | pico heap |
|---|---|---|---|---|---|---|
| `naro://start` | 0,022 ms | 0,361 ms | 0,090 ms | 0,855 ms | 54 804 | 26 329 KB |
| `rust-lang.org` | 0,488 ms | **52,060 ms** | 1,226 ms | 1,247 ms | **4 399 168** | 27 455 KB |
| MDN `/docs/Web/HTML` | 1,751 ms | **36,151 ms** | 5,163 ms | 0,618 ms | **2 061 930** | 31 937 KB |

RSS da GUI (janela aberta, página inicial): medido depois, com o mesmo método, para
comparação na seção 5.

## 3. Gargalos identificados

### 3.1 A cascata testava toda regra contra todo nó (confirmado)

`rust-lang.org` tem 2 052 regras e ~1 000 nós. O laço da cascata percorria
`sheet.rules` inteiro para cada nó: ~2 milhões de tentativas de casamento por passada de
estilo, duas passadas (fonte + resto). A instrumentação contou **5 416 casamentos
efetivos** — ou seja, 99,7 % do trabalho era descartado.

### 3.2 Cada tentativa de casamento alocava um `Vec` (confirmado)

`compound_matches` fazia `class_attr.split_whitespace().collect::<Vec<&str>>()`. Isso é
uma alocação por (regra × nó), milhões por página.

### 3.3 A folha do UA era reparseada a cada passada (confirmado)

`ua_stylesheet()` chamava `parse_stylesheet(UA_CSS)` toda vez — 60 regras parseadas por
passada de estilo, em cada mutação.

### 3.4 Cada declaração aplicada clonava a string do valor (confirmado)

`apply_declaration` fazia `decl.value.clone()` mesmo quando não havia `var()`, nas duas
passadas, e o teste de fase (`font_phase`) acontecia **depois** do clone.

### 3.5 O fallback CJK custava ~20 MB residentes sempre (confirmado)

`FontSet::load_system` abria `msyh.ttc` (19,7 MB) na inicialização, mesmo em páginas sem
um único codepoint CJK. Medido: pico de heap de 26 329 KB numa página de 1,2 KB.

### 3.6 O que **não** virou otimização

- `ComputedStyle::clone()` do estilo do pai por nó: medi 7,4–11,1 ms com e sem a mudança,
  dentro da variação. Revertido — não havia ganho comprovado.
- Layout e pintura: 1,1 ms e 0,9 ms em `rust-lang.org`. Não são gargalo neste perfil.
- O parse do CSS (4,6 ms em `rust-lang.org`) é custo real mas de outra natureza; ficou
  registrado como oportunidade, não otimizado.

## 4. Otimizações aplicadas

### 4.1 Índice de regras por tag / id / classe

`RuleIndex` agrupa as regras pelo que o **composto mais à direita** exige. `div p` só pode
casar com `<p>`; `.card` só com um nó que tenha a classe `card`. A cascata consulta
tag + id + classes do nó e recebe uma lista já em ordem de origem, sem duplicatas. Regras
sem restrição (`*`, `:hover`, `:not(.a)`) ficam no balde universal.

Semântica inalterada: o matcher já verificava tag/id/classe primeiro; o índice só evita
chamá-lo para regras que não poderiam casar. A ordem de origem é preservada porque é ela
que resolve empate de especificidade.

**A/B controlado, mesma build, mesma página:**

| Alvo | cascata sem índice | cascata com índice | ganho |
|---|---|---|---|
| `rust-lang.org` | 22,9 ms | 3,0 ms | **7,6×** |
| `mdn.html` (fixture) | 16,4 ms | 8,0 ms | **2,0×** |

(O A/B tem o custo do interruptor de ambiente nos dois lados; a build final sem o
interruptor é ainda mais rápida.)

### 4.2 Sem `Vec` por tentativa de casamento

`class_attr.split_whitespace().any(|c| c == want)` no lugar de `collect()`.

Medido na build anterior ao índice: `rust-lang.org` 52,06 → 26,21 ms, alocações
4 399 168 → 654 172 (**−85 %**).

### 4.3 Folha do UA parseada uma vez por processo

`OnceLock<Stylesheet>`. Elimina 60 regras parseadas por passada.

### 4.4 Valor da declaração emprestado (`Cow`)

O teste de fase vem antes de materializar o valor, e o caso comum empresta a string.

Medido em MDN: alocações 1 215 755 → 893 823 (−26 %), fase `apply` 6,1 → 3,7 ms.

### 4.5 Fallback de fonte sob demanda

`FontSet` guarda só a face primária e a negrito na carga; a lista de fallbacks é
preenchida na primeira vez que aparece um codepoint que a primária não cobre, com registro
dos caminhos já tentados para não reler o mesmo arquivo.

| Alvo | pico de heap antes | depois | |
|---|---|---|---|
| `naro://start` | 26 329 KB | 4 736 KB | **−82 %** |
| MDN (ao vivo) | 31 937 KB | 10 344 KB | **−68 %** |
| `rust-lang.org` | 27 455 KB | 27 508 KB | sem mudança — a página tem seletor de idiomas com CJK, então carrega a face legitimamente |
| `google.com` | — | 6 252 KB | |
| `example.com` | — | 4 684 KB | |

Verificação de que não houve perda: `tests/compat/10-text.html` continua renderizando
"日本語 简体中文 正體中文" e o heap da página CJK sobe para 26 341 KB — o fallback carrega
quando precisa, como esperado.

### 4.6 Reuso do buffer de candidatos

Um `Vec<usize>` por documento em vez de um por nó.

## 5. Resultados

### Antes / depois, mesma página

| Alvo | Métrica | Antes | Depois | Δ |
|---|---|---|---|---|
| `rust-lang.org` | cascata | 47,5 ms¹ | 2,30 ms | **20×** |
| `rust-lang.org` | alocações | 4 399 168 | 648 360 | **−85 %** |
| MDN ao vivo | cascata | 33,6 ms¹ | 11,7 ms | **2,9×** |
| MDN ao vivo | alocações | 2 061 930 | 831 262 | **−60 %** |
| MDN ao vivo | pico de heap | 31 937 KB | 10 344 KB | **−68 %** |
| `naro://start` | cascata | 0,361 ms | 0,064 ms | 5,6× |
| `naro://start` | alocações | 54 804 | 17 828 | −67 % |
| `naro://start` | pico de heap | 26 329 KB | 4 736 KB | **−82 %** |

¹ O bench antigo somava parse do CSS na fase `style`. O parse foi medido depois em
4,6 ms (`rust-lang.org`) e 2,6 ms (MDN); os valores acima descontam isso para comparar
cascata com cascata. É a única inferência da tabela, e está marcada.

### Estado final por fase

| Alvo | parse | css | style | layout | paint | alocações | pico heap |
|---|---|---|---|---|---|---|---|
| `naro://start` | 0,011 | 0,024 | 0,064 | 0,045 | 0,573 | 17 828 | 4 736 KB |
| `example.com` | 0,005 | 0,008 | 0,032 | 0,013 | 0,304 | 8 218 | 4 684 KB |
| `rust-lang.org` | 0,447 | 4,623 | 2,299 | 1,075 | 0,891 | 648 360 | 27 508 KB |
| MDN ao vivo | 1,68 | 2,61 | 11,7 | 5,0 | 0,42 | 831 262 | 10 344 KB |
| `google.com` | 0,868 | 4,787 | 2,589 | 0,631 | 1,227 | 720 477 | 6 252 KB |

### RSS da GUI

`naro` sem argumentos, janela aberta na página inicial, medido com `Get-Process`:

```
Id    RSS_MB  Private_MB
10444     19         8,9
```

Antes desta fase o mesmo processo retinha a face CJK: o pico de heap do modo headless era
26 329 KB e o RSS da GUI ficava correspondente. Não capturei o RSS da GUI no baseline, e
não vou inventar o número — a comparação válida é a do pico de heap, que é medida pelo
alocador com o mesmo método nas duas pontas.

### Correção preservada

```
cargo test --workspace     219 passed, 0 failed
naro --compat              27 pass, 0 falhas, 1 não suportados (total 28)
```

Layout de `rust-lang.org` comparado bloco a bloco com o dump anterior à fase:
**142 blocos, 142 depois, 0 diferenças de geometria.** Páginas de referência:

| Página | STATUS | CONTENT_HEIGHT |
|---|---|---|
| `example.com` | Idle | 820,60 (inalterado) |
| `rust-lang.org` | Idle | 3968,98 (inalterado) |
| `google.com` | Idle | 487,90 (inalterado) |
| MDN | Idle | 16897,00 (inalterado) |

## 6. Arquivos alterados

| Arquivo | Alteração |
|---|---|
| `crates/naro-css/src/lib.rs` | `RuleIndex` (tag/id/classe, ordem de origem, balde universal); `fill_candidates` com buffer reutilizado; `class_attr.split_whitespace().any(…)` sem `Vec`; `ua_stylesheet()` em `OnceLock`; `apply_declaration` com `Cow` e teste de fase antes da materialização |
| `crates/naro-text/src/lib.rs` | `fallbacks`/`tried_fallbacks` em `RefCell`, carregados sob demanda; `with_font_for` substitui `font_for`; `rasterize_with` separado |
| `crates/naro-engine/src/lib.rs` | `PhaseTimings::css_ms` (parse separado da cascata) |
| `crates/naro-app/src/main.rs` | imprime `css ms` no `--bench` |

Nenhum teste foi removido ou afrouxado. Nenhum relatório, captura ou benchmark anterior
foi apagado.

## 7. Limitações e oportunidades restantes

- **O parse do CSS agora é a maior fase isolada em páginas grandes** (4,6 ms em
  `rust-lang.org`, 4,8 ms no Google). O parser aloca uma `String` por nome e por valor de
  declaração. Não foi otimizado nesta fase porque não havia A/B pronto; é o próximo alvo
  natural.
- **MDN continua com 11,7 ms de cascata** para 4 327 nós e 427 regras. O índice já cortou
  o desperdício; o custo restante é aplicar declarações (parse de valores), não casar
  seletores.
- **A face CJK carrega inteira (19,7 MB) na primeira necessidade.** Carregar só a face
  necessária de um `.ttc`, ou usar um subconjunto, reduziria isso — mas exigiria suporte a
  coleções de fonte que o `ab_glyph` não expõe diretamente.
- **Sem invalidação por subárvore.** Uma mutação de atributo ainda recalcula o estilo do
  documento inteiro. O custo por passada caiu bastante com o índice, mas a fase 3 só evita
  o recálculo quando *nada* mudou. Invalidação por subárvore foi listada no plano e não
  foi feita: exigiria rastrear quais regras podem ser afetadas por cada mutação, o que é
  uma mudança de arquitetura, não uma otimização local.
- **Reuso de `Layout` em resize só de altura** também estava no plano e não foi feito: o
  `--bench` não mede resize, e sem uma medição não havia como provar ganho.
- **A contagem de alocações inclui o próprio alocador de contagem** (um `fetch_add` por
  alocação), então os tempos absolutos no modo `--bench` são ligeiramente maiores que no
  app. Os dois lados da comparação sofrem o mesmo efeito.
- **`rust-lang.org` não melhorou em memória** porque a página legitimamente usa CJK. Isso é
  o comportamento correto, não uma regressão.
