-- ============================================================================
-- MIGRATION 007: DataStream - Zero-copy ML Training Data Streaming
-- ============================================================================
-- Enables streaming data directly from CyxCloud to GPU memory during training
-- with cryptographic verification at every step.
-- ============================================================================

-- ============================================================================
-- DATASETS TABLE
-- Versioned collections of files for ML training
-- ============================================================================

CREATE TABLE datasets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(256) NOT NULL,
    owner_id UUID NOT NULL,
    description TEXT,

    -- Content addressing
    content_hash BYTEA NOT NULL,          -- Blake3 of manifest
    total_size_bytes BIGINT NOT NULL,
    file_count INTEGER NOT NULL,

    -- Schema (for structured data)
    schema JSONB,

    -- Trust: 0=self, 1=signed, 2=verified, 3=attested, 4=untrusted
    trust_level INTEGER NOT NULL DEFAULT 4,
    signature BYTEA,                       -- Ed25519 signature
    verified_at TIMESTAMP WITH TIME ZONE,

    -- Versioning
    version INTEGER NOT NULL DEFAULT 1,
    parent_version_id UUID REFERENCES datasets(id),

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(owner_id, name, version)
);

CREATE INDEX idx_datasets_owner ON datasets(owner_id);
CREATE INDEX idx_datasets_content_hash ON datasets(content_hash);
CREATE INDEX idx_datasets_trust_level ON datasets(trust_level);
CREATE INDEX idx_datasets_name ON datasets(name);

-- ============================================================================
-- DATASET_FILES TABLE
-- Files within a dataset
-- ============================================================================

CREATE TABLE dataset_files (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    file_id UUID NOT NULL REFERENCES files(id),
    path_in_dataset VARCHAR(1024) NOT NULL,
    content_hash BYTEA NOT NULL,
    size_bytes BIGINT NOT NULL,
    file_index INTEGER NOT NULL,

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(dataset_id, path_in_dataset)
);

CREATE INDEX idx_dataset_files_dataset ON dataset_files(dataset_id);
CREATE INDEX idx_dataset_files_file ON dataset_files(file_id);
CREATE INDEX idx_dataset_files_index ON dataset_files(dataset_id, file_index);

-- ============================================================================
-- PUBLIC_DATASETS TABLE
-- Registry of known public datasets (ImageNet, CIFAR, etc.)
-- ============================================================================

CREATE TABLE public_datasets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(256) NOT NULL,
    version VARCHAR(64) NOT NULL,
    official_url TEXT NOT NULL,
    official_hash BYTEA NOT NULL,
    paper_url TEXT,
    license VARCHAR(256),

    -- CyxCloud cached copy
    cached_dataset_id UUID REFERENCES datasets(id),
    cached_at TIMESTAMP WITH TIME ZONE,

    -- Verification
    verified_by TEXT[] NOT NULL DEFAULT '{}',
    verified_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(name, version)
);

CREATE INDEX idx_public_datasets_hash ON public_datasets(official_hash);
CREATE INDEX idx_public_datasets_name ON public_datasets(name);

-- ============================================================================
-- DATA_ACCESS_TOKENS TABLE
-- Short-lived tokens for Server Node direct access
-- ============================================================================

CREATE TABLE data_access_tokens (
    id UUID PRIMARY KEY,
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    node_id UUID REFERENCES nodes(id) ON DELETE CASCADE,
    user_id UUID NOT NULL,
    token_hash BYTEA NOT NULL,
    scopes TEXT[] NOT NULL,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_data_access_tokens_dataset ON data_access_tokens(dataset_id);
CREATE INDEX idx_data_access_tokens_expires ON data_access_tokens(expires_at);
CREATE INDEX idx_data_access_tokens_user ON data_access_tokens(user_id);

-- ============================================================================
-- DATASET_SHARES TABLE
-- Dataset sharing between users
-- ============================================================================

CREATE TABLE dataset_shares (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    shared_with_user_id UUID NOT NULL,
    shared_by_user_id UUID NOT NULL,
    permissions TEXT[] NOT NULL DEFAULT '{read}',  -- read, stream, reshare
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMP WITH TIME ZONE,

    UNIQUE(dataset_id, shared_with_user_id)
);

CREATE INDEX idx_dataset_shares_shared_with ON dataset_shares(shared_with_user_id);
CREATE INDEX idx_dataset_shares_shared_by ON dataset_shares(shared_by_user_id);

-- ============================================================================
-- TRIGGERS
-- ============================================================================

-- Update timestamp trigger for datasets
CREATE TRIGGER update_datasets_updated_at
    BEFORE UPDATE ON datasets
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================================
-- VIEWS
-- ============================================================================

-- View: Dataset summary with trust info
CREATE VIEW dataset_summary AS
SELECT
    d.id,
    d.name,
    d.owner_id,
    d.description,
    d.total_size_bytes,
    d.file_count,
    d.trust_level,
    d.version,
    d.created_at,
    d.updated_at,
    CASE d.trust_level
        WHEN 0 THEN 'self'
        WHEN 1 THEN 'signed'
        WHEN 2 THEN 'verified'
        WHEN 3 THEN 'attested'
        ELSE 'untrusted'
    END as trust_label,
    pd.name as public_dataset_name,
    pd.verified_by as public_verified_by
FROM datasets d
LEFT JOIN public_datasets pd ON pd.cached_dataset_id = d.id;

-- View: User's accessible datasets (owned + shared)
CREATE VIEW user_accessible_datasets AS
SELECT
    d.*,
    'owner' as access_type,
    ARRAY['read', 'write', 'stream', 'share', 'delete'] as permissions
FROM datasets d
UNION ALL
SELECT
    d.*,
    'shared' as access_type,
    ds.permissions
FROM datasets d
JOIN dataset_shares ds ON ds.dataset_id = d.id
WHERE ds.expires_at IS NULL OR ds.expires_at > NOW();

-- ============================================================================
-- SEED PUBLIC DATASETS
-- ============================================================================

INSERT INTO public_datasets (name, version, official_url, official_hash, paper_url, license, verified_by)
VALUES
    ('MNIST', '1.0', 'http://yann.lecun.com/exdb/mnist/',
     decode('0000000000000000000000000000000000000000000000000000000000000001', 'hex'),
     'http://yann.lecun.com/exdb/publis/pdf/lecun-98.pdf', 'CC BY-SA 3.0',
     ARRAY['torchvision', 'tensorflow-datasets']),

    ('CIFAR-10', '1.0', 'https://www.cs.toronto.edu/~kriz/cifar.html',
     decode('0000000000000000000000000000000000000000000000000000000000000002', 'hex'),
     'https://www.cs.toronto.edu/~kriz/learning-features-2009-TR.pdf', 'MIT',
     ARRAY['torchvision', 'tensorflow-datasets', 'huggingface']),

    ('CIFAR-100', '1.0', 'https://www.cs.toronto.edu/~kriz/cifar.html',
     decode('0000000000000000000000000000000000000000000000000000000000000003', 'hex'),
     'https://www.cs.toronto.edu/~kriz/learning-features-2009-TR.pdf', 'MIT',
     ARRAY['torchvision', 'tensorflow-datasets']),

    ('ImageNet-1K', '2012', 'https://www.image-net.org/',
     decode('0000000000000000000000000000000000000000000000000000000000000004', 'hex'),
     'https://arxiv.org/abs/1409.0575', 'Custom (research only)',
     ARRAY['torchvision', 'tensorflow-datasets', 'huggingface']),

    ('Fashion-MNIST', '1.0', 'https://github.com/zalandoresearch/fashion-mnist',
     decode('0000000000000000000000000000000000000000000000000000000000000005', 'hex'),
     'https://arxiv.org/abs/1708.07747', 'MIT',
     ARRAY['torchvision', 'tensorflow-datasets'])
ON CONFLICT (name, version) DO NOTHING;
